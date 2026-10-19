//! `notification`: NotificationManagerService for the app host
//! (`NotificationManagerService` at `android-16.0.0_r1`, ADR 0013): apps'
//! channels and groups ([`notification_channels`], persisted in
//! `notification_policy.xml`), their notifications posted, updated and
//! cancelled by the original's rules, and the listeners of the system uid
//! told of each (`registerListener`, as SystemUI registers): the Mac's
//! notification bridge (`crate::notifications`) shows them.
//!
//! A notification is kept as the app wrote it, its parts the system
//! changes (`fixNotification`: flags, the allowlist token, removed extras
//! and actions, a full-screen intent the app may not use) changed in its
//! parcel. A foreground service's notification (ActivityManager's policy
//! for it, `applyForegroundServiceNotification`) keeps its flag, cannot be
//! cleared or auto-cancelled, is not counted against its app's limit, and
//! raises a minimal or blocked channel to low; ActivityManager posts,
//! cancels and unflags it as the system. What the original does that this
//! does not: notifications of user-initiated jobs (none run in the app
//! host), zen (DND)
//! filtering beyond the manual rule, ranking by `NotificationComparator`
//! (listeners get posting order), autogrouping, snoozing, toasts, app
//! `NotificationListenerService`s and assistants, and the
//! POST_NOTIFICATIONS prompt of an app's first channel; their calls are
//! not implemented (#1488).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use aim_android_xml::{Element, Node};
use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception,
    Parcel, Reader,
};
use aim_service_aidl::android_app_inotificationmanager as nm;
use aim_service_aidl::android_service_notification_inotificationlistener as listener;
use aim_service_aidl::android_service_notification_istatusbarnotificationholder as holder;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::notification_channels::{
    CHANNEL_CLASS, Channel, DEFAULT_CHANNEL_ID, Defaults, GROUP_CLASS, Group, GroupParcel,
    IMPORTANCE_NONE, IMPORTANCE_UNSPECIFIED, Preferences, SYSTEM_RESERVED_IDS,
};
use super::services::FgsPolicy;
use super::zen_config;
use super::{AppHost, PER_USER_RANGE, reply};
use crate::clip::Raw;
use crate::notifications::parcels::{
    self, CALL_STYLE, FLAG_FOREGROUND_SERVICE, FLAG_GROUP_SUMMARY, FLAG_NO_CLEAR,
    FLAG_ONGOING_EVENT, FLAG_PROMOTED_ONGOING, Files,
};
use crate::package::list_slice::{self, Slice};

/// `Notification.FLAG_*` the service sets or clears.
const FLAG_AUTO_CANCEL: i32 = 0x10;
const FLAG_HIGH_PRIORITY: i32 = 0x80;
const FLAG_BUBBLE: i32 = 0x1000;
const FLAG_AUTOGROUP_SUMMARY: i32 = 0x400;
const FLAG_CAN_COLORIZE: i32 = 0x800;
const FLAG_NO_DISMISS: i32 = 0x2000;
const FLAG_FSI_REQUESTED_BUT_DENIED: i32 = 0x4000;
const FLAG_USER_INITIATED_JOB: i32 = 0x8000;
const FLAG_LIFETIME_EXTENDED_BY_DIRECT_REPLY: i32 = 0x10000;
/// `Notification.FOREGROUND_SERVICE_IMMEDIATE`, `FOREGROUND_SERVICE_DEFERRED`.
const FOREGROUND_SERVICE_IMMEDIATE: i32 = 1;
const FOREGROUND_SERVICE_DEFERRED: i32 = 2;
/// `Notification.DEFAULT_SOUND`, `DEFAULT_VIBRATE`.
const DEFAULT_SOUND: i32 = 1;
const DEFAULT_VIBRATE: i32 = 2;
/// `NotificationManager.IMPORTANCE_*`.
const IMPORTANCE_MIN: i32 = 1;
const IMPORTANCE_LOW: i32 = 2;
const IMPORTANCE_DEFAULT: i32 = 3;
const IMPORTANCE_HIGH: i32 = 4;
/// `NotificationChannel.USER_LOCKED_*` the importance of a record reads.
const USER_LOCKED_IMPORTANCE: i32 = 0x4;
const USER_LOCKED_VIBRATION: i32 = 0x10;
const USER_LOCKED_SOUND: i32 = 0x20;
/// `NotificationListenerService.REASON_*`.
const REASON_CLICK: i32 = 1;
const REASON_APP_CANCEL: i32 = 8;
const REASON_APP_CANCEL_ALL: i32 = 9;
const REASON_LISTENER_CANCEL: i32 = 10;
const REASON_GROUP_SUMMARY_CANCELED: i32 = 12;
const REASON_CHANNEL_REMOVED: i32 = 20;
/// `NotificationListenerService.TRIM_FULL`.
const TRIM_FULL: i32 = 0;
/// `NotificationManagerService.MAX_PACKAGE_NOTIFICATIONS`,
/// `DEFAULT_MAX_NOTIFICATION_ENQUEUE_RATE`, `NOTIFICATION_MAX_AGE_AT_POST`.
const MAX_PACKAGE_NOTIFICATIONS: usize = 50;
const DEFAULT_MAX_ENQUEUE_RATE: f32 = 5.0;
const NOTIFICATION_MAX_AGE_AT_POST: i64 = 14 * 24 * 60 * 60 * 1000;
/// `Settings.Global.ZEN_MODE_OFF`.
const ZEN_MODE_OFF: i32 = 0;
/// `UserHandle.USER_ALL`, `USER_SYSTEM`.
const USER_ALL: i32 = -1;
const USER_SYSTEM: i32 = 0;
/// `Build.VERSION_CODES.O`, `VANILLA_ICE_CREAM`
/// (`ENFORCE_NO_CLEAR_FLAG_ON_MEDIA_NOTIFICATION`).
const Q: i32 = 29;
const O: i32 = 26;
const VANILLA_ICE_CREAM: i32 = 35;
/// `Ranking`'s parcel version; `RankingMap`'s class.
const RANKING_PARCEL_VERSION: i32 = 2;
const RANKING_MAP_CLASS: &str =
    "android.service.notification.NotificationListenerService$RankingMap";
const SBN_CLASS: &str = "android.service.notification.StatusBarNotification";
/// The styles `isMediaNotification` and `canBeNonDismissible` know.
const MEDIA_STYLES: [&str; 2] = [
    "android.app.Notification$MediaStyle",
    "android.app.Notification$DecoratedMediaCustomViewStyle",
];
/// Extras only the system may set, and the permission that lets an app
/// keep each (`fixNotification`); none for those removed always.
/// The media ones only of a media style's notification.
const RESTRICTED_EXTRAS: [(&str, Option<&str>); 6] = [
    ("android.summarization", None),
    (
        "android.allowDuringSetup",
        Some("android.permission.NOTIFICATION_DURING_SETUP"),
    ),
    (
        "android.substName",
        Some("android.permission.SUBSTITUTE_NOTIFICATION_APP_NAME"),
    ),
    ("android.mediaRemoteDevice", Some(MEDIA_CONTENT_CONTROL)),
    ("android.mediaRemoteIcon", Some(MEDIA_CONTENT_CONTROL)),
    ("android.mediaRemoteIntent", Some(MEDIA_CONTENT_CONTROL)),
];
const MEDIA_CONTENT_CONTROL: &str = "android.permission.MEDIA_CONTENT_CONTROL";
/// The size of a binder object and its stability in a parcel.
const BINDER_SIZE: usize = 28;

/// Files of a notification's parcel are not read: its parts are found,
/// not its images.
struct NoFiles;

impl Files for NoFiles {
    fn read(&self, _: u32, _: usize) -> Option<Vec<u8>> {
        None
    }
}

/// A notification's parcel, its binders held.
#[derive(Clone)]
struct Kept {
    raw: Raw,
    /// Where its allowlist token and flags are.
    token_at: usize,
    flags_at: usize,
    _binders: Arc<Vec<Strong>>,
}

/// `NotificationRecord`: a posted notification.
struct Record {
    key: String,
    package: String,
    op_package: String,
    id: i32,
    tag: Option<String>,
    uid: i32,
    initial_pid: i32,
    user: i32,
    post_time: i64,
    instance_id: i32,
    notification: Arc<Kept>,
    parsed: parcels::Notification,
    channel: Channel,
    importance: i32,
    explanation: &'static str,
    noisy: bool,
    show_badge: bool,
    text_changed: bool,
}

/// A `StatusBarNotification` as written.
#[derive(Clone)]
struct Sbn {
    package: String,
    op_package: String,
    id: i32,
    tag: Option<String>,
    uid: i32,
    initial_pid: i32,
    user: i32,
    post_time: i64,
    notification: Arc<Kept>,
    /// The allowlist token, for the system's listeners; none for the app
    /// (`sanitizeSbn`).
    token: Option<Binder>,
    instance_id: Option<i32>,
}

impl Sbn {
    fn of(r: &Record, token: Option<Binder>) -> Sbn {
        Sbn {
            package: r.package.clone(),
            op_package: r.op_package.clone(),
            id: r.id,
            tag: r.tag.clone(),
            uid: r.uid,
            initial_pid: r.initial_pid,
            user: r.user,
            post_time: r.post_time,
            notification: r.notification.clone(),
            token,
            instance_id: token.map(|_| r.instance_id),
        }
    }
}

impl WriteParcelable for Sbn {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.package));
        p.write_string16(Some(&self.op_package));
        p.write_i32(self.id);
        p.write_i32(self.tag.is_some() as i32);
        if let Some(t) = &self.tag {
            p.write_string16(Some(t));
        }
        p.write_i32(self.uid);
        p.write_i32(self.initial_pid);
        let n = &self.notification;
        match self.token {
            Some(token) => {
                let mut b = Parcel::new();
                b.write_binder(Some(token));
                n.raw
                    .replace(n.token_at, n.token_at + BINDER_SIZE, &b)
                    .write(p);
            }
            None => n.raw.write(p),
        }
        p.write_i32(self.user);
        p.write_i64(self.post_time);
        p.write_i32(0); // no override group key
        p.write_i32(self.instance_id.is_some() as i32);
        if let Some(id) = self.instance_id {
            p.write_i32(id);
        }
    }
}

/// `NotificationManagerService.StatusBarNotificationHolder`: its
/// notification, given once.
struct Holder(Mutex<Option<Sbn>>);

impl Service for Holder {
    fn descriptor(&self) -> &str {
        holder::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != holder::GET {
            return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);
        }
        holder::Get::read(&mut call.data)?;
        let mut p = Parcel::new();
        holder::write_get_reply(&mut p, self.0.lock().unwrap().take().as_ref());
        Ok(p)
    }
}

/// A `Ranking` of the ranking map.
struct Ranking {
    key: String,
    rank: i32,
    matches_interruption_filter: bool,
    visibility_override: i32,
    importance: i32,
    explanation: &'static str,
    channel: Channel,
    show_badge: bool,
    noisy: bool,
    text_changed: bool,
}

impl Ranking {
    /// `Ranking.writeToParcel`, of what the service knows: no smart
    /// actions or replies, people, snooze criteria, shortcut, bubble or
    /// adjustment.
    fn write(&self, p: &mut Parcel) {
        p.write_i32(RANKING_PARCEL_VERSION);
        p.write_string16(Some(&self.key));
        p.write_i32(self.rank);
        p.write_bool(self.importance < IMPORTANCE_LOW); // ambient
        p.write_bool(self.matches_interruption_filter);
        p.write_i32(self.visibility_override);
        p.write_i32(0); // suppressed visual effects
        p.write_i32(self.importance);
        crate::clip::write_char_sequence(p, Some(self.explanation));
        p.write_f32(0.0); // ranking score
        p.write_string16(None); // override group key
        p.write_string16(Some(CHANNEL_CLASS));
        self.channel.write_to(p);
        p.write_i32(-1); // override people
        p.write_i32(-1); // snooze criteria
        p.write_bool(self.show_badge);
        p.write_i32(0); // USER_SENTIMENT_NEUTRAL
        p.write_bool(false); // hidden
        p.write_i64(0); // last audibly alerted
        p.write_bool(self.noisy);
        p.write_i32(-1); // smart actions
        p.write_i32(-1); // smart replies
        p.write_bool(false); // can bubble
        p.write_bool(self.text_changed);
        p.write_bool(false); // conversation
        p.write_string16(None); // shortcut info
        p.write_i32(0); // RANKING_UNCHANGED
        p.write_bool(false); // bubble
        p.write_i32(IMPORTANCE_UNSPECIFIED); // proposed importance
        p.write_bool(false); // sensitive content
        p.write_string16(None); // summarization
    }
}

/// `NotificationRankingUpdate` with `ranking_update_ashmem`: the ranking
/// map in read-only shared memory, and the (no) smart actions.
struct RankingUpdate(aim_binder_driver::File);

impl RankingUpdate {
    fn new(rankings: &[Ranking]) -> Result<RankingUpdate, Exception> {
        let mut map = Parcel::new();
        map.write_string16(Some(RANKING_MAP_CLASS));
        map.write_i32(rankings.len() as i32);
        for r in rankings {
            r.write(&mut map);
        }
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let memory = super::bind::read_only_memory(map.data())
            .map_err(|e| state(format!("ranking map: {e}")))?;
        let file = aim_binder_host::server::file_from_fd(std::os::fd::AsFd::as_fd(&memory))
            .ok_or_else(|| state("ranking map: no fileport".into()))?;
        Ok(RankingUpdate(file))
    }
}

impl WriteParcelable for RankingUpdate {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some("android.os.SharedMemory"));
        p.write_file(self.0.clone());
        p.write_i32(0); // an empty bundle of smart actions
    }
}

/// `NotificationStats` of a notification nobody saw or touched.
struct Stats;

impl WriteParcelable for Stats {
    fn write_to(&self, p: &mut Parcel) {
        // seen, expanded, direct replied, smart replied, snoozed, viewed
        // settings, interacted
        for _ in 0..7 {
            p.write_i32(0);
        }
        p.write_i32(-1); // DISMISSAL_NOT_DISMISSED
        p.write_i32(-1000); // DISMISS_SENTIMENT_UNKNOWN
    }
}

/// A listener of the system (`ManagedServices.ManagedServiceInfo`).
struct Listener {
    binder: Arc<Strong>,
    user: i32,
    death: u64,
}

/// `RateEstimator`: an app's rate of posts.
struct Rate {
    last: Option<i64>,
    interarrival: f64,
}

impl Default for Rate {
    fn default() -> Self {
        Rate {
            last: None,
            interarrival: 1.0,
        }
    }
}

impl Rate {
    fn estimate(&self, now: i64, last: i64) -> f64 {
        let dt = ((now - last) as f64 / 1000.0).max(0.0005);
        0.7 * self.interarrival + 0.3 * dt
    }

    fn update(&mut self, now: i64) {
        if let Some(last) = self.last {
            self.interarrival = self.estimate(now, last);
        }
        self.last = Some(now);
    }

    fn rate(&self, now: i64) -> f32 {
        self.last
            .map_or(0.0, |last| (1.0 / self.estimate(now, last)) as f32)
    }
}

struct State {
    path: PathBuf,
    /// `notification_policy.xml`'s root, its ranking rewritten on saves.
    policy: Element,
    prefs: Preferences,
    zen_mode: i32,
    /// `mNotificationList`, in posting order.
    records: Vec<Record>,
    listeners: Vec<Listener>,
    rates: HashMap<String, Rate>,
    next_instance: i32,
}

pub struct NotificationManager {
    host: Arc<AppHost>,
    /// `ALLOWLIST_TOKEN`.
    token: Binder,
    state: Mutex<Option<State>>,
}

/// What a cancel matches (`cancelAllNotificationsInt`'s and
/// `cancelNotification`'s arguments).
struct Cancel<'a> {
    package: Option<&'a str>,
    tag: Option<Option<&'a str>>,
    id: Option<i32>,
    channel: Option<&'a str>,
    must_have: i32,
    must_not_have: i32,
    user: i32,
    reason: i32,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn elapsed_ms() -> i64 {
    super::clock_ms(libc::CLOCK_MONOTONIC_RAW)
}

/// `StatusBarNotification.key()`.
fn key(user: i32, package: &str, id: i32, tag: Option<&str>, uid: i32) -> String {
    format!("{user}|{package}|{id}|{}|{uid}", tag.unwrap_or("null"))
}

fn empty(r: Result<(), Exception>) -> Reply {
    reply(r.map(|()| {
        let mut p = Parcel::new();
        p.write_no_exception();
        p
    }))
}

fn unsupported(what: &str) -> Exception {
    Exception::new(
        EX_UNSUPPORTED_OPERATION,
        format!("notification: {what} is not implemented (#1488)"),
    )
}

impl NotificationManager {
    pub fn new(host: Arc<AppHost>) -> NotificationManager {
        NotificationManager {
            token: host.process.add_service(Arc::new(super::activity::Token)),
            host,
            state: Mutex::new(None),
        }
    }

    /// The service's state, read from `notification_policy.xml` the first
    /// time.
    fn state(&self) -> Result<MutexGuard<'_, Option<State>>, Exception> {
        let mut guard = self.state.lock().unwrap();
        if guard.is_none() {
            let inputs = self.host.inputs()?;
            let path = inputs.data.join("system/notification_policy.xml");
            let fail =
                |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
            let policy = match std::fs::read(&path) {
                Ok(bytes) => aim_android_xml::read(&bytes).map_err(fail)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Element {
                    name: "notification-policy".into(),
                    attrs: vec![("version".into(), aim_android_xml::Value::Int(1))],
                    content: Vec::new(),
                },
                Err(e) => return Err(fail(e.to_string())),
            };
            let label = self.default_label()?;
            let prefs = match policy.children().find(|e| e.name == "ranking") {
                Some(ranking) => {
                    let pre_o = self.pre_o()?;
                    Preferences::read(
                        ranking,
                        &Defaults {
                            pre_o: &pre_o,
                            label: &label,
                        },
                        now_ms(),
                    )
                }
                None => Preferences::default(),
            };
            // ZenModeHelper's manual rule: automatic rules, whose state
            // their condition providers set, are not run (#1488).
            let zen_mode = policy
                .children()
                .find(|e| e.name == "zen")
                .and_then(|z| z.children().find(|e| e.name == "manual"))
                .and_then(|m| m.int("zen").ok().flatten())
                .unwrap_or(ZEN_MODE_OFF);
            *guard = Some(State {
                path,
                policy,
                prefs,
                zen_mode,
                records: Vec::new(),
                listeners: Vec::new(),
                rates: HashMap::new(),
                next_instance: 1,
            });
        }
        Ok(guard)
    }

    /// The rules `uid` may see after `enforcePolicyAccess(uid, method)`:
    /// those `canManageAutomaticZenRule` lets it manage. The app host runs
    /// user 0 alone, the zen user of every caller (`getCallingZenUser`).
    fn manageable_zen_rules(
        &self,
        uid: i32,
        method: &str,
    ) -> Result<Vec<zen_config::ZenRule>, Exception> {
        let manage = self
            .host
            .check_permission(Some("android.permission.MANAGE_NOTIFICATIONS"), uid)?
            == 0;
        let packages = self.packages_for_uid(uid)?;
        let guard = self.state()?;
        let policy = &guard.as_ref().unwrap().policy;
        let user = uid / PER_USER_RANGE as i32;
        if !manage
            && !packages
                .iter()
                .any(|p| zen_config::policy_access_approved(policy, p, user))
        {
            eprintln!("app host: Notification policy access denied calling {method}");
            return Err(Exception::security("Notification policy access denied"));
        }
        Ok(zen_config::read(policy)?
            .rules()
            .into_iter()
            .filter(|r| {
                uid == 0
                    || uid == 1000
                    || manage
                    || r.pkg.as_ref().is_some_and(|p| packages.contains(p))
            })
            .collect())
    }

    /// `getAutomaticZenRules`.
    fn automatic_zen_rules(&self, uid: i32) -> Result<Vec<zen_config::AzrWithId>, Exception> {
        zen_config::automatic_zen_rules(self.manageable_zen_rules(uid, "getAutomaticZenRules")?)
    }

    /// `getAutomaticZenRule`: the rule of `id`, if `uid` may manage it.
    fn automatic_zen_rule(
        &self,
        uid: i32,
        id: Option<&str>,
    ) -> Result<Option<zen_config::AutomaticZenRule>, Exception> {
        let id = id.ok_or_else(|| Exception::new(EX_NULL_POINTER, "Id is null"))?;
        self.manageable_zen_rules(uid, "getAutomaticZenRule")?
            .into_iter()
            .find(|r| r.id.as_deref() == Some(id))
            .map(zen_config::automatic_zen_rule)
            .transpose()
    }

    /// `getZenModeConfig`: the system or the phone, else STATUS_BAR_SERVICE.
    fn zen_mode_config(&self, uid: i32) -> Result<zen_config::ZenModeConfig, Exception> {
        let system_or_phone = uid == 0 || matches!(uid % PER_USER_RANGE as i32, 1000 | 1001);
        if !system_or_phone
            && self
                .host
                .check_permission(Some("android.permission.STATUS_BAR_SERVICE"), uid)?
                != 0
        {
            return Err(Exception::security(format!(
                "INotificationManager.getZenModeConfig: uid {uid} does not have android.permission.STATUS_BAR_SERVICE."
            )));
        }
        let guard = self.state()?;
        zen_config::read(&guard.as_ref().unwrap().policy)
    }

    /// `getPackagesForUid`.
    fn packages_for_uid(&self, uid: i32) -> Result<Vec<String>, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let packages = query.packages_for_uid(uid).map_err(|e| {
            Exception::new(EX_UNSUPPORTED_OPERATION, format!("notification: {}", e.0))
        })?;
        Ok(packages.into_iter().flatten().flatten().collect())
    }

    /// `R.string.default_notification_channel_label`.
    fn default_label(&self) -> Result<String, Exception> {
        self.host
            .inputs()?
            .packages
            .framework_string("default_notification_channel_label")
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "no default_notification_channel_label")
            })
    }

    /// Whether a package targets an SDK before O.
    fn pre_o(&self) -> Result<impl Fn(&str) -> bool + use<>, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(move |name: &str| {
            state
                .packages
                .get(name)
                .is_some_and(|p| p.target_sdk_version < O)
        })
    }

    fn target_sdk(&self, package: &str) -> Result<i32, Exception> {
        let (state, _) = self.host.packages()?;
        state
            .packages
            .get(package)
            .map(|p| p.target_sdk_version)
            .ok_or_else(|| Exception::illegal_argument(format!("Unknown package {package}")))
    }

    /// Runs `f` on the preferences with their defaults.
    fn with_prefs<T>(
        &self,
        f: impl FnOnce(&mut State, &Defaults<'_>) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let label = self.default_label()?;
        let pre_o = self.pre_o()?;
        let mut guard = self.state()?;
        let state = guard.as_mut().unwrap();
        f(
            state,
            &Defaults {
                pre_o: &pre_o,
                label: &label,
            },
        )
    }

    /// `handleSavePolicyFile`: the file with the ranking written now, as
    /// `AtomicFile` writes it.
    fn save(state: &mut State) -> Result<(), Exception> {
        let ranking = state.prefs.to_xml();
        let content = &mut state.policy.content;
        match content
            .iter_mut()
            .find(|n| matches!(n, Node::Element(e) if e.name == "ranking"))
        {
            Some(n) => *n = Node::Element(ranking),
            None => content.push(Node::Element(ranking)),
        }
        let fail =
            |e: String| Exception::new(EX_ILLEGAL_STATE, format!("notification_policy.xml: {e}"));
        let bytes = aim_android_xml::abx::write(&state.policy).map_err(fail)?;
        let new = state.path.with_extension("xml.new");
        let write = || -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&new)?;
            std::io::Write::write_all(&mut file, &bytes)?;
            crate::fsync(&file)?;
            aim_storage::guest_inode::record(
                &new,
                aim_storage::guest_inode::GuestInode {
                    uid: Some(1000),
                    gid: Some(1000),
                    mode: Some(0o600),
                },
            )?;
            std::fs::rename(&new, &state.path)
        };
        write().map_err(|e| fail(e.to_string()))
    }

    /// `checkCallerIsSystemOrSameApp`.
    fn check_system_or_same_app(&self, uid: i32, package: &str) -> Result<(), Exception> {
        if is_system(uid) {
            return Ok(());
        }
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security(format!(
                "Package {package} does not belong to {uid}"
            )));
        }
        Ok(())
    }

    /// `enforceSystemOrSystemUI`.
    fn enforce_system_or_system_ui(&self, uid: i32, message: &str) -> Result<(), Exception> {
        if is_system(uid)
            || self
                .host
                .check_permission(Some("android.permission.STATUS_BAR_SERVICE"), uid)?
                == 0
        {
            return Ok(());
        }
        Err(Exception::security(message.to_string()))
    }

    /// `PermissionHelper.hasPermission`: POST_NOTIFICATIONS.
    fn notifications_allowed(&self, uid: i32) -> Result<bool, Exception> {
        Ok(self
            .host
            .check_permission(Some("android.permission.POST_NOTIFICATIONS"), uid)?
            == 0)
    }

    /// The uid of `package` in `user`.
    fn package_uid(&self, package: &str, user: i32) -> Result<Option<i32>, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state
            .packages
            .get(package)
            .map(|p| user.max(0) * PER_USER_RANGE as i32 + p.app_id))
    }

    /// `resolveNotificationUid`: the caller's own package, or any as the
    /// system; delegates are not supported (#1488).
    fn notification_uid(
        &self,
        op_package: &str,
        package: &str,
        calling_uid: i32,
        user: i32,
    ) -> Result<i32, Exception> {
        let user = if user == USER_ALL { USER_SYSTEM } else { user };
        if calling_uid / PER_USER_RANGE as i32 == user
            && self.host.owns_package(calling_uid, package)?
        {
            return Ok(calling_uid);
        }
        if (is_system(calling_uid) || op_package == "android")
            && let Some(uid) = self.package_uid(package, user)?
        {
            return Ok(uid);
        }
        Err(Exception::security(format!(
            "Caller {op_package}:{calling_uid} trying to post for invalid pkg {package} in user {user}"
        )))
    }

    fn enqueue(&self, call: &mut Call<'_>) -> Result<(), Exception> {
        let bad = |_| Exception::illegal_argument("enqueueNotificationWithTag");
        let r = &mut call.data;
        r.enforce_interface(nm::DESCRIPTOR).map_err(bad)?;
        let package = r.read_string16().map_err(bad)?;
        let op_package = r.read_string16().map_err(bad)?;
        let tag = r.read_string16().map_err(bad)?;
        let id = r.read_i32().map_err(bad)?;
        let present = r.read_i32().map_err(bad)? != 0;
        let (Some(package), true) = (package, present) else {
            return Err(Exception::illegal_argument(format!(
                "null not allowed: pkg=null id={id} notification=null"
            )));
        };
        let op_package = op_package.unwrap_or_else(|| package.clone());
        let incoming = Incoming::read(r, &self.host.process)?;
        let incoming_user = r.read_i32().map_err(bad)?;
        self.enqueue_internal(Enqueue {
            package,
            op_package,
            calling_uid: call.sender_euid as i32,
            pid: call.sender_pid,
            tag,
            id,
            notification: incoming,
            incoming_user,
        })
    }

    /// `enqueueNotificationInternal`, of an app's notification or of a
    /// foreground service's (`NotificationManagerInternal
    /// .enqueueNotification`, which ActivityManager posts as the app).
    pub(super) fn enqueue_internal(&self, e: Enqueue) -> Result<(), Exception> {
        let user = self
            .host
            .incoming_user(e.calling_uid, e.incoming_user, true)?;
        let uid = self.notification_uid(&e.op_package, &e.package, e.calling_uid, user)?;
        let mut n = e.notification.n.clone();
        if n.allowlist_token.is_some_and(|t| t != self.token) {
            return Err(Exception::security(format!(
                "Unexpected allowlist token received from {}",
                e.calling_uid
            )));
        }
        // ActivityManager's policy for a foreground service's
        // notification, asked outside the service's locks.
        let policy = self.host.apply_foreground_service_notification(
            &e.notification,
            e.tag.as_deref(),
            e.id,
            &e.package,
            user,
        );
        let target_sdk = self.target_sdk(&e.package)?;
        let fixes = self.fixes(
            &n,
            &e.package,
            uid,
            target_sdk,
            policy != FgsPolicy::NotForegroundService,
        )?;
        let (raw, flags_at) = apply(e.notification.raw.clone(), &mut n, &fixes)?;
        let process = &self.host.process;
        let binders = raw.handles().map(|h| process.strong(h)).collect();
        let kept = Kept {
            raw,
            token_at: n.layout.token,
            flags_at,
            _binders: Arc::new(binders),
        };
        if policy == FgsPolicy::UpdateOnly {
            // Posted only as an update of what shows; the service posts it
            // once its deferral ends.
            let key = key(user, &e.package, e.id, e.tag.as_deref(), uid);
            if !self
                .state()?
                .as_ref()
                .unwrap()
                .records
                .iter()
                .any(|r| r.key == key)
            {
                self.host
                    .foreground_service_notification_updated(false, e.id, &e.package, user);
                return Ok(());
            }
        }
        self.post(Post {
            package: e.package,
            op_package: e.op_package,
            tag: e.tag,
            id: e.id,
            uid,
            pid: e.pid,
            user,
            target_sdk,
            notification: n,
            kept,
        })
    }

    /// `fixNotification`'s decisions of a notification of `package`, a
    /// foreground service's or not (`fgs`, ActivityManager's policy).
    fn fixes(
        &self,
        n: &parcels::Notification,
        package: &str,
        uid: i32,
        target_sdk: i32,
        fgs: bool,
    ) -> Result<Fixes, Exception> {
        let held = |p: &str| self.host.check_permission(Some(p), uid).map(|r| r == 0);
        let style = n.extras.template.as_deref();
        let media_style = style.is_some_and(|s| MEDIA_STYLES.contains(&s));
        if style == Some(CALL_STYLE) {
            // CallStyle's system actions (getActionsListWithSystemActions).
            return Err(unsupported("a CallStyle notification"));
        }
        let mut flags = n.flags;
        // A foreground service's notification is flagged as one
        // (`applyForegroundServiceNotificationLocked`), any other not. No
        // user-initiated job runs in the app host.
        if fgs {
            flags |= FLAG_FOREGROUND_SERVICE;
        } else {
            flags &= !FLAG_FOREGROUND_SERVICE;
        }
        flags &= !FLAG_USER_INITIATED_JOB;
        if flags & (FLAG_FOREGROUND_SERVICE | FLAG_USER_INITIATED_JOB) != 0 {
            flags &= !FLAG_AUTO_CANCEL;
        }
        flags &= !FLAG_LIFETIME_EXTENDED_BY_DIRECT_REPLY;
        // canBeNonDismissible: a media notification, or the default search
        // selector's; there are no enterprise admins.
        let search_selector = self
            .host
            .inputs()?
            .packages
            .framework_string("config_defaultSearchSelectorPackageName")
            .is_some_and(|s| s == package);
        if flags & FLAG_ONGOING_EVENT != 0
            && ((media_style && n.extras.media_session) || search_selector)
        {
            flags |= FLAG_NO_DISMISS;
        } else {
            flags &= !FLAG_NO_DISMISS;
        }
        if held("android.permission.USE_COLORIZED_NOTIFICATIONS")? {
            flags |= FLAG_CAN_COLORIZE;
        } else {
            flags &= !FLAG_CAN_COLORIZE;
        }
        flags &= !FLAG_PROMOTED_ONGOING;
        if media_style && target_sdk >= VANILLA_ICE_CREAM {
            flags |= FLAG_NO_CLEAR;
        }
        let mut extras = Vec::new();
        for (key, permission) in RESTRICTED_EXTRAS {
            let remove = match permission {
                None => true,
                Some(MEDIA_CONTENT_CONTROL) => media_style && !held(MEDIA_CONTENT_CONTROL)?,
                Some(permission) => !held(permission)?,
            };
            if remove {
                extras.push(key);
            }
        }
        let mut sticky_hun = false;
        if n.full_screen_intent.is_some() {
            // checkUseFullScreenIntentPermission: the permission, and its
            // op allowed or left to the permission.
            let op = super::appops_service::mode(
                &self.host,
                super::appops_service::OP_USE_FULL_SCREEN_INTENT,
                uid,
                Some(package),
            )?;
            let can = target_sdk < Q
                || (held("android.permission.USE_FULL_SCREEN_INTENT")?
                    && matches!(op, 0 | super::appops_service::MODE_DEFAULT));
            if !can {
                // makeStickyHun.
                sticky_hun = true;
                if self
                    .requested_permission(package, "android.permission.USE_FULL_SCREEN_INTENT")?
                {
                    flags |= FLAG_FSI_REQUESTED_BUT_DENIED;
                }
            }
        }
        Ok(Fixes {
            flags,
            extras,
            sticky_hun,
        })
    }

    /// `PermissionHelper.hasRequestedPermission`.
    fn requested_permission(&self, package: &str, permission: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state
            .packages
            .get(package)
            .and_then(|p| p.pkg.as_ref())
            .is_some_and(|pkg| pkg.requested_permissions.iter().any(|r| r == permission)))
    }

    /// The rest of `enqueueNotificationInternal`, then
    /// `EnqueueNotificationRunnable` and `PostNotificationRunnable`.
    fn post(&self, p: Post) -> Result<(), Exception> {
        let label = self.default_label()?;
        let pre_o = self.pre_o()?;
        let allowed = self.notifications_allowed(p.uid)?;
        let max_rate = self
            .host
            .inputs()?
            .settings
            .get(
                super::settings::Table::Global,
                "max_notification_enqueue_rate",
            )
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_ENQUEUE_RATE);
        let mut guard = self.state()?;
        let state = guard.as_mut().unwrap();
        let defaults = Defaults {
            pre_o: &pre_o,
            label: &label,
        };
        let n = p.notification;
        let key = key(p.user, &p.package, p.id, p.tag.as_deref(), p.uid);
        // getNotificationChannelRestoreDeleted.
        let channel_id = n.channel_id.as_deref();
        if channel_id.is_some_and(|c| SYSTEM_RESERVED_IDS.contains(&c)) {
            return Ok(());
        }
        let conversation = n.shortcut_id.as_deref();
        let Some(channel) = state.prefs.conversation_channel(
            &p.package,
            p.uid,
            channel_id,
            conversation,
            true,
            conversation.is_some_and(|c| !c.is_empty()),
            &defaults,
        ) else {
            // The original toasts a developer warning when notifications
            // are allowed.
            eprintln!(
                "app host: notification: No Channel found for pkg={}, channelId={}, id={}, tag={:?}",
                p.package,
                channel_id.unwrap_or("null"),
                p.id,
                p.tag
            );
            return Ok(());
        };
        if channel.deleted {
            if conversation != channel.conversation_id.as_deref() {
                return Ok(());
            }
            if state
                .prefs
                .create_channel(
                    &p.package,
                    p.uid,
                    channel.clone(),
                    true,
                    false,
                    true,
                    &defaults,
                )
                .map_err(Exception::illegal_argument)?
            {
                Self::save(state)?;
            }
        }
        let mut n = n;
        // apiRichOngoing: canBePromoted (the default; ui_rich_ongoing is
        // off) and the notification's characteristics.
        if n.has_promotable_characteristics() && channel.importance > IMPORTANCE_MIN {
            n.flags |= FLAG_PROMOTED_ONGOING;
        }
        let mut kept = p.kept;
        kept.raw.set_i32(kept.flags_at, n.flags);
        // NotificationRecord: its importance, sound and vibration.
        let mut channel = channel;
        let pre_channels = channel.id == DEFAULT_CHANNEL_ID && p.target_sdk < O;
        let user_importance = channel.user_locked_fields & USER_LOCKED_IMPORTANCE != 0;
        let sound = if pre_channels && channel.user_locked_fields & USER_LOCKED_SOUND == 0 {
            n.defaults & DEFAULT_SOUND != 0 || n.sound
        } else {
            channel.sound.is_some()
        };
        let vibration = if pre_channels && channel.user_locked_fields & USER_LOCKED_VIBRATION == 0 {
            n.defaults & DEFAULT_VIBRATE != 0 || n.vibrate
        } else {
            channel.vibration_enabled
        };
        let noisy = sound || vibration;
        let (mut importance, mut explanation) =
            if pre_channels && (channel.importance == IMPORTANCE_UNSPECIFIED || !user_importance) {
                let priority = if n.flags & FLAG_HIGH_PRIORITY != 0 {
                    2
                } else {
                    n.priority.clamp(-2, 2)
                };
                let mut requested = match priority {
                    -2 => IMPORTANCE_MIN,
                    -1 => IMPORTANCE_LOW,
                    0 => IMPORTANCE_DEFAULT,
                    _ => IMPORTANCE_HIGH,
                };
                if !noisy && requested > IMPORTANCE_LOW {
                    requested = IMPORTANCE_LOW;
                }
                if noisy && requested < IMPORTANCE_DEFAULT {
                    requested = IMPORTANCE_DEFAULT;
                }
                if n.full_screen_intent.is_some() {
                    requested = IMPORTANCE_HIGH;
                }
                (requested, "app")
            } else {
                (
                    channel.importance,
                    if user_importance { "user" } else { "app" },
                )
            };
        // A foreground service's or job's notification of a minimal or
        // blocked importance is raised to low, unless the user chose it
        // after the channel showed one.
        let fgs_or_uij = n.flags & (FLAG_FOREGROUND_SERVICE | FLAG_USER_INITIATED_JOB) != 0;
        if fgs_or_uij {
            if (!user_importance || !channel.user_visible_task_shown)
                && matches!(importance, IMPORTANCE_MIN | IMPORTANCE_NONE)
            {
                channel.importance = IMPORTANCE_LOW;
                (importance, explanation) = (IMPORTANCE_LOW, "system");
                if !channel.user_visible_task_shown {
                    channel.user_locked_fields &= !USER_LOCKED_IMPORTANCE;
                    channel.user_visible_task_shown = true;
                }
                state
                    .prefs
                    .update_channel(&p.package, p.uid, &mut channel)
                    .map_err(channel_error)?;
                Self::save(state)?;
            } else if !channel.user_visible_task_shown
                && n.channel_id
                    .as_deref()
                    .is_some_and(|c| !c.is_empty() && c != DEFAULT_CHANNEL_ID)
            {
                channel.user_visible_task_shown = true;
            }
        }
        // checkDisqualifyingFeatures.
        let system = is_system(p.uid) || p.package == "android";
        let exists = state.records.iter().any(|r| r.key == key);
        if !system {
            let rate = state.rates.entry(p.package.clone()).or_default();
            let completed_progress = n.extras.progress_max.is_some_and(|max| max > 0)
                && n.extras.progress == n.extras.progress_max;
            if exists && !completed_progress && rate.rate(elapsed_ms()) > max_rate {
                eprintln!(
                    "app host: notification: Shedding {key}. package={}",
                    p.package
                );
                return Ok(());
            }
            // A foreground service's or job's is not counted against it.
            let count = state
                .records
                .iter()
                .filter(|r| r.package == p.package && r.user == p.user && r.key != key)
                .count();
            if !fgs_or_uij && count >= MAX_PACKAGE_NOTIFICATIONS {
                eprintln!(
                    "app host: notification: Package has already posted or enqueued {count} notifications.  Not showing more.  package={}",
                    p.package
                );
                return Ok(());
            }
        }
        let media = n
            .extras
            .template
            .as_deref()
            .is_some_and(|s| MEDIA_STYLES.contains(&s))
            && n.extras.media_session;
        let blocked = !allowed
            || channel.importance == IMPORTANCE_NONE
            || state
                .prefs
                .group_blocked(&p.package, p.uid, channel.group.as_deref());
        if blocked && !media {
            return Ok(());
        }
        // rejectOldNotifications, of a `when` the app set (`hasAppProvidedWhen`).
        let app_when = n.when != 0 && n.when != n.creation_time;
        if app_when && n.when > 0 && now_ms() - n.when > NOTIFICATION_MAX_AGE_AT_POST {
            return Ok(());
        }
        if !system {
            state
                .rates
                .entry(p.package.clone())
                .or_default()
                .update(elapsed_ms());
        }
        let old = state.records.iter().position(|r| r.key == key);
        if let Some(at) = old {
            // A foreground service's notification stays one when updated.
            n.flags |= state.records[at].parsed.flags & FLAG_FOREGROUND_SERVICE;
        }
        if n.flags & FLAG_FOREGROUND_SERVICE != 0 {
            n.flags |= FLAG_NO_CLEAR;
        }
        kept.raw.set_i32(kept.flags_at, n.flags);
        // isVisuallyInterruptive, of the texts it compares; a foreground
        // service's or job's update is not.
        let text_changed = old.is_none_or(|at| {
            let o = &state.records[at].parsed.extras;
            let e = &n.extras;
            n.flags & (FLAG_FOREGROUND_SERVICE | FLAG_USER_INITIATED_JOB) == 0
                && n.flags & parcels::FLAG_ONLY_ALERT_ONCE == 0
                && (o.title != e.title
                    || o.text != e.text
                    || o.big_text != e.big_text
                    || o.lines != e.lines)
        });
        let instance_id = state.next_instance;
        state.next_instance += 1;
        let show_badge = state.prefs.can_show_badge(&p.package, p.uid) && channel.show_badge;
        let record = Record {
            key: key.clone(),
            package: p.package,
            op_package: p.op_package,
            id: p.id,
            tag: p.tag,
            uid: p.uid,
            initial_pid: p.pid,
            user: p.user,
            post_time: now_ms(),
            instance_id,
            notification: Arc::new(kept),
            parsed: n,
            channel,
            importance,
            explanation,
            noisy,
            show_badge,
            text_changed,
        };
        let previous_summary = old.and_then(|at| {
            let r = &state.records[at];
            (r.parsed.flags & FLAG_GROUP_SUMMARY != 0).then(|| r.parsed.group.clone())
        });
        match old {
            Some(at) => state.records[at] = record,
            None => state.records.push(record),
        }
        let at = old.unwrap_or(state.records.len() - 1);
        // A summary that stops being one, or changes group, takes its
        // children (`PostNotificationRunnable`'s group cancel).
        if let Some(group) = previous_summary.flatten() {
            let r = &state.records[at];
            if r.parsed.flags & FLAG_GROUP_SUMMARY == 0 || r.parsed.group.as_deref() != Some(&group)
            {
                let (package, user) = (r.package.clone(), r.user);
                self.cancel_children(state, &package, user, &group, REASON_GROUP_SUMMARY_CANCELED);
            }
        }
        self.notify_posted(state, at);
        // maybeReportForegroundServiceUpdate, outside the lock.
        let r = &state.records[at];
        let fgs = (r.parsed.flags & FLAG_FOREGROUND_SERVICE != 0)
            .then(|| (r.id, r.package.clone(), r.user));
        drop(guard);
        if let Some((id, package, user)) = fgs {
            self.host
                .foreground_service_notification_updated(true, id, &package, user);
        }
        Ok(())
    }

    /// `NotificationManagerInternal.cancelNotification` of a foreground
    /// service's notification, as the system.
    pub(super) fn cancel_foreground_service_notification(
        &self,
        package: &str,
        id: i32,
        user: i32,
    ) -> Result<(), Exception> {
        let mut guard = self.state()?;
        self.cancel(
            guard.as_mut().unwrap(),
            &Cancel {
                package: Some(package),
                tag: Some(None),
                id: Some(id),
                channel: None,
                must_have: 0,
                must_not_have: 0,
                user,
                reason: REASON_APP_CANCEL,
            },
        );
        Ok(())
    }

    /// `removeForegroundServiceFlagFromNotification`: the notification of
    /// a service no longer foreground loses its flag, its listeners told,
    /// or goes if the package is over its count.
    pub(super) fn remove_foreground_service_flag(
        &self,
        package: &str,
        id: i32,
        user: i32,
    ) -> Result<(), Exception> {
        let mut guard = self.state()?;
        let state = guard.as_mut().unwrap();
        let count = state
            .records
            .iter()
            .filter(|r| r.package == package && r.user == user)
            .count();
        let Some(at) = state
            .records
            .iter()
            .position(|r| r.package == package && r.tag.is_none() && r.id == id && r.user == user)
        else {
            return Ok(());
        };
        if count > MAX_PACKAGE_NOTIFICATIONS {
            let r = state.records.remove(at);
            self.notify_removed(state, &r, REASON_APP_CANCEL);
            return Ok(());
        }
        let r = &mut state.records[at];
        r.parsed.flags &= !FLAG_FOREGROUND_SERVICE;
        let mut kept = Kept::clone(&r.notification);
        kept.raw.set_i32(kept.flags_at, r.parsed.flags);
        r.notification = Arc::new(kept);
        self.notify_posted(state, at);
        Ok(())
    }

    /// Tells the listeners of the post of `state.records[at]`.
    fn notify_posted(&self, state: &State, at: usize) {
        let record = &state.records[at];
        for l in state
            .listeners
            .iter()
            .filter(|l| l.user == USER_ALL || l.user == record.user)
        {
            let update = match self.ranking_update(state, l.user) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("app host: notification: ranking update: {}", e.message);
                    return;
                }
            };
            let holder = self
                .host
                .process
                .add_service(Arc::new(Holder(Mutex::new(Some(Sbn::of(
                    record,
                    Some(self.token),
                ))))));
            let mut data = Parcel::new();
            listener::OnNotificationPosted {
                notification_holder: Some(holder),
                update: Some(update),
            }
            .write(&mut data);
            if let Err(s) = l
                .binder
                .transact(listener::ON_NOTIFICATION_POSTED, &data, true)
            {
                eprintln!("app host: notification: unable to notify listener (posted): status {s}");
            }
        }
    }

    /// Tells the listeners of the removal of `record`.
    fn notify_removed(&self, state: &State, record: &Record, reason: i32) {
        for l in state
            .listeners
            .iter()
            .filter(|l| l.user == USER_ALL || l.user == record.user)
        {
            let update = match self.ranking_update(state, l.user) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("app host: notification: ranking update: {}", e.message);
                    return;
                }
            };
            let holder = self
                .host
                .process
                .add_service(Arc::new(Holder(Mutex::new(Some(Sbn::of(
                    record,
                    Some(self.token),
                ))))));
            let mut data = Parcel::new();
            listener::OnNotificationRemoved {
                notification_holder: Some(holder),
                update: Some(update),
                stats: Some(Stats),
                reason,
            }
            .write(&mut data);
            if let Err(s) = l
                .binder
                .transact(listener::ON_NOTIFICATION_REMOVED, &data, true)
            {
                eprintln!(
                    "app host: notification: unable to notify listener (removed): status {s}"
                );
            }
        }
    }

    /// `makeRankingUpdateLocked` for a listener of `user`.
    fn ranking_update(&self, state: &State, user: i32) -> Result<RankingUpdate, Exception> {
        let rankings: Vec<Ranking> = state
            .records
            .iter()
            .filter(|r| user == USER_ALL || r.user == user)
            .enumerate()
            .map(|(rank, r)| Ranking {
                key: r.key.clone(),
                rank: rank as i32,
                // Not intercepted unless zen is on (ZenModeFiltering is
                // not run, #1488).
                matches_interruption_filter: state.zen_mode == ZEN_MODE_OFF,
                visibility_override: r.channel.lockscreen_visibility,
                importance: r.importance,
                explanation: r.explanation,
                channel: r.channel.clone(),
                show_badge: r.show_badge,
                noisy: r.noisy,
                text_changed: r.text_changed,
            })
            .collect();
        RankingUpdate::new(&rankings)
    }

    /// `cancelGroupChildrenLocked`.
    fn cancel_children(
        &self,
        state: &mut State,
        package: &str,
        user: i32,
        group: &str,
        reason: i32,
    ) {
        let mut at = 0;
        while at < state.records.len() {
            let r = &state.records[at];
            let child = r.package == package
                && r.user == user
                && r.parsed.group.as_deref() == Some(group)
                && r.parsed.flags & FLAG_GROUP_SUMMARY == 0
                && r.parsed.flags & (FLAG_FOREGROUND_SERVICE | FLAG_USER_INITIATED_JOB) == 0;
            if child {
                let r = state.records.remove(at);
                self.notify_removed(state, &r, reason);
            } else {
                at += 1;
            }
        }
    }

    /// `cancelNotification` and `cancelAllNotificationsInt`: the records
    /// `c` matches go, a summary's children with it.
    fn cancel(&self, state: &mut State, c: &Cancel<'_>) {
        let mut at = 0;
        let mut summaries = Vec::new();
        while at < state.records.len() {
            let r = &state.records[at];
            let matches = c.package.is_none_or(|p| r.package == p)
                && (c.user == USER_ALL || r.user == USER_ALL || r.user == c.user)
                && c.tag.is_none_or(|t| r.tag.as_deref() == t)
                && c.id.is_none_or(|id| r.id == id)
                && c.channel.is_none_or(|ch| r.channel.id == ch)
                && r.parsed.flags & c.must_have == c.must_have
                && r.parsed.flags & c.must_not_have == 0;
            if !matches {
                at += 1;
                continue;
            }
            let r = state.records.remove(at);
            if r.parsed.flags & FLAG_GROUP_SUMMARY != 0
                && let Some(g) = &r.parsed.group
            {
                summaries.push((r.package.clone(), r.user, g.clone()));
            }
            self.notify_removed(state, &r, c.reason);
        }
        for (package, user, group) in summaries {
            self.cancel_children(state, &package, user, &group, REASON_GROUP_SUMMARY_CANCELED);
        }
    }

    /// `NotificationDelegate.onNotificationClick`: an auto-cancel
    /// notification goes, unless a foreground service's, a job's or a
    /// bubble.
    pub(super) fn clicked(&self, key: &str) -> Result<(), Exception> {
        let mut guard = self.state()?;
        let state = guard.as_mut().unwrap();
        let Some(r) = state.records.iter().find(|r| r.key == key) else {
            return Ok(());
        };
        let (package, tag, id, user) = (r.package.clone(), r.tag.clone(), r.id, r.user);
        self.cancel(
            state,
            &Cancel {
                package: Some(&package),
                tag: Some(tag.as_deref()),
                id: Some(id),
                channel: None,
                must_have: FLAG_AUTO_CANCEL,
                must_not_have: FLAG_FOREGROUND_SERVICE | FLAG_USER_INITIATED_JOB | FLAG_BUBBLE,
                user,
                reason: REASON_CLICK,
            },
        );
        Ok(())
    }

    fn register_listener(&self, call: &mut Call<'_>) -> Result<(), Exception> {
        let args = nm::RegisterListener::<Skip>::read(&mut call.data)
            .map_err(|_| Exception::illegal_argument("registerListener"))?;
        self.enforce_system_or_system_ui(
            call.sender_euid as i32,
            "INotificationManager.registerListener",
        )?;
        let Some(Binder::Handle(handle)) = args.listener else {
            return Err(Exception::illegal_argument("no listener"));
        };
        let binder = Arc::new(self.host.process.strong(handle));
        let me = Arc::downgrade(&self.host);
        let death = self.host.process.link_to_death(
            &binder,
            Box::new(move || {
                if let Some(nms) = me
                    .upgrade()
                    .and_then(|h| h.notification_service.get().cloned())
                    && let Ok(mut guard) = nms.state()
                    && let Some(state) = guard.as_mut()
                {
                    state.listeners.retain(|l| l.binder.handle != handle);
                }
            }),
        );
        let mut guard = self.state()?;
        let state = guard.as_mut().unwrap();
        if state.listeners.iter().any(|l| l.binder.handle == handle) {
            self.host.process.clear_death(&binder, death);
            return Ok(());
        }
        state.listeners.push(Listener {
            binder: binder.clone(),
            user: args.userid,
            death,
        });
        let update = self.ranking_update(state, args.userid)?;
        let mut data = Parcel::new();
        listener::OnListenerConnected {
            update: Some(update),
        }
        .write(&mut data);
        if let Err(s) = binder.transact(listener::ON_LISTENER_CONNECTED, &data, true) {
            eprintln!("app host: notification: unable to notify listener (connected): status {s}");
        }
        Ok(())
    }

    /// `checkServiceTokenLocked`: the listener of `token`.
    fn listener_user(state: &State, token: Option<Binder>) -> Result<i32, Exception> {
        match token {
            Some(Binder::Handle(h)) => state
                .listeners
                .iter()
                .find(|l| l.binder.handle == h)
                .map(|l| l.user)
                .ok_or_else(|| {
                    Exception::security("Disallowed call from unknown notification listener")
                }),
            _ => Err(Exception::security(
                "Disallowed call from unknown notification listener",
            )),
        }
    }

    fn active_from_listener(&self, call: &mut Call<'_>) -> Result<Parcel, Exception> {
        let args = nm::GetActiveNotificationsFromListener::read(&mut call.data)
            .map_err(|_| Exception::illegal_argument("getActiveNotificationsFromListener"))?;
        if args.trim != TRIM_FULL {
            return Err(unsupported("a light trim"));
        }
        let guard = self.state()?;
        let state = guard.as_ref().unwrap();
        let user = Self::listener_user(state, args.token)?;
        let visible = |r: &&Record| user == USER_ALL || r.user == user;
        let sbns: Vec<Sbn> = match &args.keys {
            Some(keys) => keys
                .iter()
                .flatten()
                .filter_map(|k| state.records.iter().find(|r| &r.key == k))
                .filter(visible)
                .map(|r| Sbn::of(r, Some(self.token)))
                .collect(),
            None => state
                .records
                .iter()
                .filter(visible)
                .map(|r| Sbn::of(r, Some(self.token)))
                .collect(),
        };
        let mut p = Parcel::new();
        p.write_no_exception();
        aim_service_aidl::write_typed(
            &mut p,
            Some(&Slice::new(self.host.process.clone(), SBN_CLASS, sbns)),
        );
        Ok(p)
    }

    fn cancel_from_listener(&self, call: &mut Call<'_>) -> Result<(), Exception> {
        let args = nm::CancelNotificationsFromListener::read(&mut call.data)
            .map_err(|_| Exception::illegal_argument("cancelNotificationsFromListener"))?;
        let mut guard = self.state()?;
        let state = guard.as_mut().unwrap();
        let user = Self::listener_user(state, args.token)?;
        let Some(keys) = args.keys else {
            // A listener's "clear all".
            return Err(unsupported("cancelling every notification from a listener"));
        };
        for k in keys.iter().flatten() {
            let Some(r) = state.records.iter().find(|r| &r.key == k) else {
                continue;
            };
            if r.user != user && r.user != USER_ALL && user != USER_ALL {
                continue;
            }
            // cancelNotificationFromListenerLocked: not ongoing, not a
            // foreground service's, not lifetime-extended.
            let (package, tag, id, user) = (r.package.clone(), r.tag.clone(), r.id, r.user);
            self.cancel(
                state,
                &Cancel {
                    package: Some(&package),
                    tag: Some(tag.as_deref()),
                    id: Some(id),
                    channel: None,
                    must_have: 0,
                    must_not_have: FLAG_ONGOING_EVENT
                        | FLAG_NO_CLEAR
                        | FLAG_LIFETIME_EXTENDED_BY_DIRECT_REPLY,
                    user,
                    reason: REASON_LISTENER_CANCEL,
                },
            );
        }
        Ok(())
    }

    /// `createNotificationChannels`, or the system's
    /// `createNotificationChannelsForPackage` of a uid it names.
    fn create_channels(&self, call: &mut Call<'_>, for_package: bool) -> Result<(), Exception> {
        let bad = |_| Exception::illegal_argument("createNotificationChannels");
        let calling_uid = call.sender_euid as i32;
        let r = &mut call.data;
        r.enforce_interface(nm::DESCRIPTOR).map_err(bad)?;
        let package = r.read_string16().map_err(bad)?.unwrap_or_default();
        let uid = if for_package {
            r.read_i32().map_err(bad)?
        } else {
            calling_uid
        };
        if r.read_i32().map_err(bad)? == 0 {
            return Err(Exception::new(EX_NULL_POINTER, "channelsList"));
        }
        let channels: Vec<Channel> = list_slice::read(r, &self.host.process).map_err(bad)?;
        if for_package {
            self.enforce_system_or_system_ui(calling_uid, "only system can call this")?;
        } else {
            self.check_system_or_same_app(uid, &package)?;
        }
        if let Some(c) = channels.iter().find(|c| !c.persistable()) {
            return Err(unsupported(&format!(
                "channel {}'s vibration effect, which no pattern matches",
                c.id
            )));
        }
        let system = is_system(calling_uid);
        self.with_prefs(|state, defaults| {
            let mut changed = false;
            for c in channels {
                changed |= state
                    .prefs
                    .create_channel(&package, uid, c, true, false, system, defaults)
                    .map_err(channel_error)?;
            }
            if changed {
                Self::save(state)?;
            }
            Ok(())
        })
    }

    fn create_groups(&self, call: &mut Call<'_>) -> Result<(), Exception> {
        let bad = |_| Exception::illegal_argument("createNotificationChannelGroups");
        let r = &mut call.data;
        r.enforce_interface(nm::DESCRIPTOR).map_err(bad)?;
        let package = r.read_string16().map_err(bad)?.unwrap_or_default();
        if r.read_i32().map_err(bad)? == 0 {
            return Err(Exception::new(EX_NULL_POINTER, "channelGroupList"));
        }
        let groups: Vec<Group> = list_slice::read(r, &self.host.process).map_err(bad)?;
        let uid = call.sender_euid as i32;
        self.check_system_or_same_app(uid, &package)?;
        self.with_prefs(|state, defaults| {
            for g in groups {
                state
                    .prefs
                    .create_group(&package, uid, g, true, defaults)
                    .map_err(channel_error)?;
            }
            Self::save(state)
        })
    }

    /// `getConversationNotificationChannel` (and `getNotificationChannel`):
    /// the caller's own package's, or any as the system.
    #[allow(clippy::too_many_arguments)]
    fn channel_of(
        &self,
        call: &Call<'_>,
        calling_package: Option<&str>,
        user: i32,
        target: Option<&str>,
        channel: Option<&str>,
        conversation: Option<&str>,
        return_parent: bool,
    ) -> Result<Option<Channel>, Exception> {
        let uid = call.sender_euid as i32;
        let target = target.unwrap_or_default();
        let own = calling_package == Some(target) && self.host.owns_package(uid, target)?;
        if !own && !is_system(uid) {
            return Err(Exception::security(format!(
                "Pkg {} cannot read channels for {target} in {user}",
                calling_package.unwrap_or("null")
            )));
        }
        let target_uid = self.package_uid(target, user)?.unwrap_or(-1);
        self.with_prefs(|state, defaults| {
            Ok(state.prefs.conversation_channel(
                target,
                target_uid,
                channel,
                conversation,
                return_parent,
                false,
                defaults,
            ))
        })
    }

    /// `NotificationManagerInternal.getNotificationChannel`: whether
    /// `package` of `uid` has the channel `id` (the default one for none).
    pub(super) fn has_channel(
        &self,
        package: &str,
        uid: i32,
        id: Option<&str>,
    ) -> Result<bool, Exception> {
        self.with_prefs(|state, defaults| {
            Ok(state
                .prefs
                .conversation_channel(package, uid, id, None, true, false, defaults)
                .is_some())
        })
    }

    fn delete_channel(&self, call: &Call<'_>, package: &str, id: &str) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        self.check_system_or_same_app(uid, package)?;
        if id == DEFAULT_CHANNEL_ID {
            return Err(Exception::illegal_argument("Cannot delete default channel"));
        }
        if SYSTEM_RESERVED_IDS.contains(&id) {
            return Ok(());
        }
        let user = uid / PER_USER_RANGE as i32;
        let mut guard = self.state()?;
        let state = guard.as_mut().unwrap();
        self.cancel(
            state,
            &Cancel {
                package: Some(package),
                tag: None,
                id: None,
                channel: Some(id),
                must_have: 0,
                must_not_have: 0,
                user,
                reason: REASON_CHANNEL_REMOVED,
            },
        );
        if state.prefs.delete_channel(package, uid, id, now_ms()) {
            Self::save(state)?;
        }
        Ok(())
    }

    /// `setNotificationDelegate`: `checkCallerIsSameApp`, then the
    /// delegate (by its `ApplicationInfo` as the caller sees it, none if
    /// it does not) set, or revoked without one.
    fn set_delegate(
        &self,
        call: &Call<'_>,
        package: Option<String>,
        delegate: Option<String>,
    ) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        let package = package.unwrap_or_default();
        if !(uid == 0 && package == "root") && !self.host.owns_package(uid, &package)? {
            return Err(Exception::security(format!(
                "Package {package} is not owned by uid {uid}"
            )));
        }
        let delegate_uid = match &delegate {
            None => None,
            Some(delegate) => {
                /// `MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE`.
                const FLAGS: i64 = 0x0004_0000 | 0x0008_0000;
                let (state, resolution) = self.host.packages()?;
                let query = crate::package::query::Query {
                    state: &state,
                    filter: &resolution.apps_filter,
                    calling_uid: uid,
                };
                let info = query
                    .application_info(delegate, FLAGS, uid / PER_USER_RANGE as i32)
                    .map_err(|e| {
                        Exception::new(EX_UNSUPPORTED_OPERATION, format!("notification: {}", e.0))
                    })??;
                match info {
                    Some(info) => Some(info.uid),
                    None => return Ok(()),
                }
            }
        };
        self.with_prefs(|state, defaults| {
            match (&delegate, delegate_uid) {
                (Some(delegate), Some(delegate_uid)) => {
                    state
                        .prefs
                        .set_delegate(&package, uid, delegate, delegate_uid, defaults)
                }
                _ => state.prefs.revoke_delegate(&package, uid),
            }
            Self::save(state)
        })
    }

    fn delete_group(&self, call: &Call<'_>, package: &str, id: &str) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        self.check_system_or_same_app(uid, package)?;
        let user = uid / PER_USER_RANGE as i32;
        let mut guard = self.state()?;
        let state = guard.as_mut().unwrap();
        if state
            .prefs
            .group_with_channels(package, uid, id, false)
            .is_none()
        {
            return Ok(());
        }
        for c in state.prefs.delete_group(package, uid, id, now_ms()) {
            self.cancel(
                state,
                &Cancel {
                    package: Some(package),
                    tag: None,
                    id: None,
                    channel: Some(&c.id),
                    must_have: 0,
                    must_not_have: 0,
                    user,
                    reason: REASON_CHANNEL_REMOVED,
                },
            );
        }
        Self::save(state)
    }

    /// `getAppActiveNotifications`: the package's own, sanitized.
    fn app_active(&self, call: &Call<'_>, package: &str, user: i32) -> Result<Parcel, Exception> {
        let uid = call.sender_euid as i32;
        self.check_system_or_same_app(uid, package)?;
        let user = self.host.incoming_user(uid, user, true)?;
        let guard = self.state()?;
        let sbns: Vec<Sbn> = guard
            .as_ref()
            .unwrap()
            .records
            .iter()
            .filter(|r| r.user == user && (r.package == package || r.op_package == package))
            .map(|r| Sbn::of(r, None))
            .collect();
        let mut p = Parcel::new();
        p.write_no_exception();
        aim_service_aidl::write_typed(
            &mut p,
            Some(&Slice::new(self.host.process.clone(), SBN_CLASS, sbns)),
        );
        Ok(p)
    }
}

/// A notification as its sender parcelled it, its binders and files
/// held: what was read of it, and its parcel (which its layout is in).
#[derive(Clone)]
pub(super) struct Incoming {
    n: parcels::Notification,
    raw: Raw,
    _binders: Arc<Vec<Strong>>,
}

impl Incoming {
    /// `Notification(Parcel)`, read from `r` after its parcelable's
    /// presence, the binders and files of it held in `process`.
    pub(super) fn read(r: &mut Reader<'_>, process: &LocalProcess) -> Result<Incoming, Exception> {
        let start = r.position();
        let mut n = parcels::Notification::default();
        let read = parcels::read_notification(r, &NoFiles, &mut n);
        if read.is_err() || !n.complete {
            return Err(unsupported("a notification this service cannot read"));
        }
        let mut raw = Raw::capture(r, start);
        // Positions in the notification's own parcel.
        let layout = &mut n.layout;
        for at in [
            &mut layout.token,
            &mut layout.content_intent,
            &mut layout.flags,
            &mut layout.full_screen_intent,
            &mut layout.extras,
            &mut layout.actions,
        ] {
            *at -= start;
        }
        for at in &mut layout.null_actions {
            *at -= start;
        }
        raw.keep_files(&|fd| process.file(fd))
            .map_err(|_| Exception::illegal_argument("a notification's file"))?;
        let binders = raw.handles().map(|h| process.strong(h)).collect();
        Ok(Incoming {
            n,
            raw,
            _binders: Arc::new(binders),
        })
    }

    /// It flagged `FLAG_FOREGROUND_SERVICE`.
    pub(super) fn foreground_service(&self) -> Incoming {
        let mut fgs = self.clone();
        fgs.n.flags |= FLAG_FOREGROUND_SERVICE;
        fgs.raw.set_i32(fgs.n.layout.flags, fgs.n.flags);
        fgs
    }

    pub(super) fn has_small_icon(&self) -> bool {
        self.n.small_icon.is_some()
    }

    pub(super) fn channel_id(&self) -> Option<&str> {
        self.n.channel_id.as_deref()
    }

    /// `Notification.isForegroundDisplayForceDeferred`.
    pub(super) fn force_deferred(&self) -> bool {
        self.n.fgs_defer_behavior == FOREGROUND_SERVICE_DEFERRED
    }

    /// `Notification.shouldShowForegroundImmediately`: asked to, or not
    /// asked to defer and a media notification, a call's, navigation's or
    /// one with actions.
    pub(super) fn show_foreground_immediately(&self) -> bool {
        let n = &self.n;
        match n.fgs_defer_behavior {
            FOREGROUND_SERVICE_IMMEDIATE => true,
            FOREGROUND_SERVICE_DEFERRED => false,
            _ => {
                let media = n
                    .extras
                    .template
                    .as_deref()
                    .is_some_and(|s| MEDIA_STYLES.contains(&s))
                    && n.extras.media_session;
                media
                    || matches!(n.category.as_deref(), Some("call" | "navigation"))
                    || !n.actions.is_empty()
            }
        }
    }
}

/// What `enqueue_internal` enqueues.
pub(super) struct Enqueue {
    pub package: String,
    pub op_package: String,
    pub calling_uid: i32,
    pub pid: i32,
    pub tag: Option<String>,
    pub id: i32,
    pub notification: Incoming,
    pub incoming_user: i32,
}

/// What `post` posts.
struct Post {
    package: String,
    op_package: String,
    tag: Option<String>,
    id: i32,
    uid: i32,
    pid: i32,
    user: i32,
    target_sdk: i32,
    notification: parcels::Notification,
    kept: Kept,
}

/// What `fixNotification` changes of a notification: its flags, the
/// extras it removes (of those the notification has), and whether its
/// full-screen intent becomes its content intent (`makeStickyHun`).
struct Fixes {
    flags: i32,
    extras: Vec<&'static str>,
    sticky_hun: bool,
}

/// `fixes` made to the notification's parcel `raw`, of which `n` was read
/// (and its layout is in): the parcel, and where its flags are now.
fn apply(
    mut raw: Raw,
    n: &mut parcels::Notification,
    fixes: &Fixes,
) -> Result<(Raw, usize), Exception> {
    let layout = n.layout.clone();
    // Replacements of ranges, made from the end of the parcel so that
    // earlier positions hold.
    let mut edits: Vec<(usize, usize, Parcel)> = Vec::new();
    let int = |v: i32| {
        let mut p = Parcel::new();
        p.write_i32(v);
        p
    };
    // Null actions dropped.
    for &at in &layout.null_actions {
        edits.push((at, at + 4, Parcel::new()));
    }
    if !layout.null_actions.is_empty() {
        edits.push((
            layout.actions,
            layout.actions + 4,
            int(n.actions.len() as i32),
        ));
    }
    let parcel = raw.parcel();
    let mut reader = parcel.reader();
    reader.set_position(layout.extras);
    let entries = parcels::bundle_entries(&mut reader).map_err(|_| unsupported("its extras"))?;
    let removed: Vec<(usize, usize)> = entries
        .iter()
        .filter(|(key, ..)| fixes.extras.contains(&key.as_str()))
        .map(|(_, from, to)| (*from, *to))
        .collect();
    if !removed.is_empty() {
        // The bundle's length (of what follows its magic) and count.
        let at = layout.extras;
        let length = i32::from_le_bytes(parcel.data()[at..at + 4].try_into().unwrap());
        let gone: usize = removed.iter().map(|(f, t)| t - f).sum();
        edits.push((at, at + 4, int(length - gone as i32)));
        edits.push((at + 8, at + 12, int((entries.len() - removed.len()) as i32)));
        edits.extend(
            removed
                .into_iter()
                .map(|(from, to)| (from, to, Parcel::new())),
        );
    }
    let mut flags_at = layout.flags;
    if let (true, Some(fsi)) = (fixes.sticky_hun, n.full_screen_intent) {
        let at = layout.full_screen_intent;
        edits.push((at, at + 4 + BINDER_SIZE, int(0)));
        if n.content_intent.is_none() {
            let mut intent = int(1);
            intent.write_binder(Some(fsi));
            edits.push((layout.content_intent, layout.content_intent + 4, intent));
            flags_at += BINDER_SIZE;
            n.content_intent = Some(fsi);
        }
        n.full_screen_intent = None;
    }
    raw.set_i32(layout.flags, fixes.flags);
    n.flags = fixes.flags;
    edits.sort_by_key(|(from, ..)| std::cmp::Reverse(*from));
    for (from, to, with) in edits {
        raw = raw.replace(from, to, &with);
    }
    Ok((raw, flags_at))
}

/// A parcelable read past (the listener's `ComponentName`).
struct Skip;

impl ReadParcelable for Skip {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        r.read_string16()?;
        r.read_string16()?;
        Ok(Skip)
    }
}

/// `Process.SYSTEM_UID`, `PHONE_UID` and root: `isUidSystemOrPhone`.
fn is_system(uid: i32) -> bool {
    matches!(uid.rem_euclid(PER_USER_RANGE as i32), 0 | 1000 | 1001)
}

/// A `PreferencesHelper` error as the exception it throws.
fn channel_error(e: String) -> Exception {
    match e.split_once(": ") {
        Some(("IllegalStateException", m)) => Exception::new(EX_ILLEGAL_STATE, m.to_string()),
        Some(("NullPointerException", m)) => Exception::new(EX_NULL_POINTER, m.to_string()),
        Some((_, m)) => Exception::illegal_argument(m.to_string()),
        None => Exception::illegal_argument(e),
    }
}

impl Service for NotificationManager {
    fn descriptor(&self) -> &str {
        nm::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        // A notification's bitmaps in ashmem.
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let bool_reply = |r: Result<bool, Exception>| {
            reply(r.map(|v| {
                let mut p = Parcel::new();
                p.write_no_exception();
                p.write_bool(v);
                p
            }))
        };
        match call.code {
            nm::ENQUEUE_NOTIFICATION_WITH_TAG => empty(self.enqueue(call)),
            nm::CANCEL_NOTIFICATION_WITH_TAG => {
                let a =
                    nm::CancelNotificationWithTag::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = a.pkg.unwrap_or_default();
                let op = a.op_pkg.unwrap_or_else(|| package.clone());
                empty((|| {
                    let user = self.host.incoming_user(uid, a.user_id, true)?;
                    let notification_uid = self.notification_uid(&op, &package, uid, user)?;
                    let _ = notification_uid;
                    let must_not_have = if is_system(uid) {
                        0
                    } else {
                        FLAG_FOREGROUND_SERVICE
                            | FLAG_USER_INITIATED_JOB
                            | FLAG_AUTOGROUP_SUMMARY
                            | FLAG_LIFETIME_EXTENDED_BY_DIRECT_REPLY
                    };
                    let mut guard = self.state()?;
                    self.cancel(
                        guard.as_mut().unwrap(),
                        &Cancel {
                            package: Some(&package),
                            tag: Some(a.tag.as_deref()),
                            id: Some(a.id),
                            channel: None,
                            must_have: 0,
                            must_not_have,
                            user,
                            reason: REASON_APP_CANCEL,
                        },
                    );
                    Ok(())
                })())
            }
            nm::CANCEL_ALL_NOTIFICATIONS => {
                let a = nm::CancelAllNotifications::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = a.pkg.unwrap_or_default();
                empty((|| {
                    self.check_system_or_same_app(uid, &package)?;
                    let user = self.host.incoming_user(uid, a.user_id, true)?;
                    let mut guard = self.state()?;
                    self.cancel(
                        guard.as_mut().unwrap(),
                        &Cancel {
                            package: Some(&package),
                            tag: None,
                            id: None,
                            channel: None,
                            must_have: 0,
                            must_not_have: FLAG_FOREGROUND_SERVICE
                                | FLAG_USER_INITIATED_JOB
                                | FLAG_LIFETIME_EXTENDED_BY_DIRECT_REPLY,
                            user,
                            reason: REASON_APP_CANCEL_ALL,
                        },
                    );
                    Ok(())
                })())
            }
            nm::ARE_NOTIFICATIONS_ENABLED => {
                let a = nm::AreNotificationsEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bool_reply((|| {
                    let package = a.pkg.unwrap_or_default();
                    if !is_system(uid) && !self.host.owns_package(uid, &package)? {
                        return Err(Exception::security(
                            "Caller not system or systemui or same package",
                        ));
                    }
                    self.notifications_allowed(uid)
                })())
            }
            nm::ARE_NOTIFICATIONS_ENABLED_FOR_PACKAGE => {
                let a = nm::AreNotificationsEnabledForPackage::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                bool_reply((|| {
                    let package = a.pkg.unwrap_or_default();
                    if !is_system(uid) && !self.host.owns_package(uid, &package)? {
                        return Err(Exception::security(
                            "Caller not system or systemui or same package",
                        ));
                    }
                    if uid / PER_USER_RANGE as i32 != a.uid / PER_USER_RANGE as i32
                        && self.host.check_permission(
                            Some("android.permission.INTERACT_ACROSS_USERS"),
                            uid,
                        )? != 0
                    {
                        return Err(Exception::security(format!(
                            "canNotifyAsPackage for uid {}",
                            a.uid
                        )));
                    }
                    self.notifications_allowed(a.uid)
                })())
            }
            nm::GET_PACKAGE_IMPORTANCE => {
                let a = nm::GetPackageImportance::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    self.check_system_or_same_app(uid, &a.pkg.unwrap_or_default())?;
                    let importance = if self.notifications_allowed(uid)? {
                        IMPORTANCE_DEFAULT
                    } else {
                        IMPORTANCE_NONE
                    };
                    let mut p = Parcel::new();
                    nm::write_get_package_importance_reply(&mut p, importance);
                    Ok(p)
                })())
            }
            nm::CREATE_NOTIFICATION_CHANNELS => empty(self.create_channels(call, false)),
            nm::CREATE_NOTIFICATION_CHANNELS_FOR_PACKAGE => empty(self.create_channels(call, true)),
            nm::CREATE_NOTIFICATION_CHANNEL_GROUPS => empty(self.create_groups(call)),
            nm::GET_NOTIFICATION_CHANNEL => {
                let a = nm::GetNotificationChannel::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.channel_of(
                        call,
                        a.calling_pkg.as_deref(),
                        a.user_id,
                        a.pkg.as_deref(),
                        a.channel_id.as_deref(),
                        None,
                        true,
                    )
                    .map(|c| {
                        let mut p = Parcel::new();
                        nm::write_get_notification_channel_reply(&mut p, c.as_ref());
                        p
                    }),
                )
            }
            nm::GET_CONVERSATION_NOTIFICATION_CHANNEL => {
                let a = nm::GetConversationNotificationChannel::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(
                    self.channel_of(
                        call,
                        a.calling_pkg.as_deref(),
                        a.user_id,
                        a.pkg.as_deref(),
                        a.channel_id.as_deref(),
                        a.conversation_id.as_deref(),
                        a.return_parent_if_no_conversation_channel,
                    )
                    .map(|c| {
                        let mut p = Parcel::new();
                        nm::write_get_conversation_notification_channel_reply(&mut p, c.as_ref());
                        p
                    }),
                )
            }
            nm::GET_NOTIFICATION_CHANNEL_FOR_PACKAGE => {
                let a = nm::GetNotificationChannelForPackage::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply((|| {
                    if !is_system(uid) {
                        return Err(Exception::security("Disallowed call for uid"));
                    }
                    let c = self.with_prefs(|state, defaults| {
                        Ok(state.prefs.conversation_channel(
                            &a.pkg.unwrap_or_default(),
                            a.uid,
                            a.channel_id.as_deref(),
                            a.conversation_id.as_deref(),
                            true,
                            a.include_deleted,
                            defaults,
                        ))
                    })?;
                    let mut p = Parcel::new();
                    nm::write_get_notification_channel_for_package_reply(&mut p, c.as_ref());
                    Ok(p)
                })())
            }
            nm::GET_NOTIFICATION_CHANNELS => {
                let a = nm::GetNotificationChannels::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let target = a.target_pkg.unwrap_or_default();
                    let own = a.calling_pkg.as_deref() == Some(&target)
                        && self.host.owns_package(uid, &target)?;
                    if !own && !is_system(uid) {
                        return Err(Exception::security(format!(
                            "Pkg {} cannot read channels for {target} in {}",
                            a.calling_pkg.as_deref().unwrap_or("null"),
                            a.user_id
                        )));
                    }
                    let target_uid = self.package_uid(&target, a.user_id)?.unwrap_or(-1);
                    let channels = self.with_prefs(|state, defaults| {
                        Ok(state
                            .prefs
                            .channels(&target, target_uid, false, true, defaults))
                    })?;
                    let mut p = Parcel::new();
                    p.write_no_exception();
                    aim_service_aidl::write_typed(
                        &mut p,
                        Some(&Slice::new(
                            self.host.process.clone(),
                            CHANNEL_CLASS,
                            channels,
                        )),
                    );
                    Ok(p)
                })())
            }
            nm::DELETE_NOTIFICATION_CHANNEL => {
                let a =
                    nm::DeleteNotificationChannel::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                empty(self.delete_channel(
                    call,
                    &a.pkg.unwrap_or_default(),
                    &a.channel_id.unwrap_or_default(),
                ))
            }
            nm::SET_NOTIFICATION_DELEGATE => {
                let a = nm::SetNotificationDelegate::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                empty(self.set_delegate(call, a.calling_pkg, a.delegate))
            }
            nm::DELETE_NOTIFICATION_CHANNEL_GROUP => {
                let a = nm::DeleteNotificationChannelGroup::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                empty(self.delete_group(
                    call,
                    &a.pkg.unwrap_or_default(),
                    &a.channel_group_id.unwrap_or_default(),
                ))
            }
            nm::GET_NOTIFICATION_CHANNEL_GROUP => {
                let a =
                    nm::GetNotificationChannelGroup::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let package = a.pkg.unwrap_or_default();
                    self.check_system_or_same_app(uid, &package)?;
                    let group = self.with_prefs(|state, _| {
                        Ok(state.prefs.group_with_channels(
                            &package,
                            uid,
                            &a.channel_group_id.unwrap_or_default(),
                            false,
                        ))
                    })?;
                    let mut p = Parcel::new();
                    nm::write_get_notification_channel_group_reply(
                        &mut p,
                        group
                            .map(|group| GroupParcel {
                                group,
                                process: self.host.process.clone(),
                            })
                            .as_ref(),
                    );
                    Ok(p)
                })())
            }
            nm::GET_NOTIFICATION_CHANNEL_GROUPS
            | nm::GET_NOTIFICATION_CHANNEL_GROUPS_WITHOUT_CHANNELS => {
                let code = call.code;
                let a = nm::GetNotificationChannelGroups::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let package = a.pkg.unwrap_or_default();
                    self.check_system_or_same_app(uid, &package)?;
                    let groups = self.with_prefs(|state, _| {
                        Ok(if code == nm::GET_NOTIFICATION_CHANNEL_GROUPS {
                            state.prefs.groups_with_channels(&package, uid)
                        } else {
                            state
                                .prefs
                                .package(&package, uid)
                                .map(|p| p.groups.clone())
                                .unwrap_or_default()
                        })
                    })?;
                    let groups = groups
                        .into_iter()
                        .map(|group| GroupParcel {
                            group,
                            process: self.host.process.clone(),
                        })
                        .collect();
                    let mut p = Parcel::new();
                    p.write_no_exception();
                    aim_service_aidl::write_typed(
                        &mut p,
                        Some(&Slice::new(self.host.process.clone(), GROUP_CLASS, groups)),
                    );
                    Ok(p)
                })())
            }
            nm::GET_APP_ACTIVE_NOTIFICATIONS => {
                let a =
                    nm::GetAppActiveNotifications::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.app_active(call, &a.calling_pkg.unwrap_or_default(), a.user_id))
            }
            // `enforceSystemOrSystemUI`, then a copy of the config.
            nm::GET_ZEN_MODE_CONFIG => {
                nm::GetZenModeConfig::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.zen_mode_config(call.sender_euid as i32).map(|config| {
                    let mut p = Parcel::new();
                    nm::write_get_zen_mode_config_reply(
                        &mut p,
                        Some(&zen_config::ConfigParcel(config, self.host.process.clone())),
                    );
                    p
                }))
            }
            nm::GET_AUTOMATIC_ZEN_RULES => {
                nm::GetAutomaticZenRules::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.automatic_zen_rules(call.sender_euid as i32)
                        .map(|rules| {
                            let mut p = Parcel::new();
                            nm::write_get_automatic_zen_rules_reply(
                                &mut p,
                                Some(&Slice::new(
                                    self.host.process.clone(),
                                    zen_config::AZR_WITH_ID_CLASS,
                                    rules,
                                )),
                            );
                            p
                        }),
                )
            }
            nm::GET_AUTOMATIC_ZEN_RULE => {
                let a = nm::GetAutomaticZenRule::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.automatic_zen_rule(call.sender_euid as i32, a.id.as_deref())
                        .map(|rule| {
                            let mut p = Parcel::new();
                            nm::write_get_automatic_zen_rule_reply(&mut p, rule.as_ref());
                            p
                        }),
                )
            }
            nm::GET_ZEN_MODE => {
                nm::GetZenMode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.state().map(|guard| {
                    let mut p = Parcel::new();
                    nm::write_get_zen_mode_reply(&mut p, guard.as_ref().unwrap().zen_mode);
                    p
                }))
            }
            nm::REGISTER_LISTENER => empty(self.register_listener(call)),
            nm::UNREGISTER_LISTENER => {
                let a = nm::UnregisterListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                empty((|| {
                    let mut guard = self.state()?;
                    let state = guard.as_mut().unwrap();
                    if let Some(Binder::Handle(h)) = a.listener
                        && let Some(at) = state.listeners.iter().position(|l| l.binder.handle == h)
                    {
                        let l = state.listeners.remove(at);
                        self.host.process.clear_death(&l.binder, l.death);
                    }
                    Ok(())
                })())
            }
            nm::GET_ACTIVE_NOTIFICATIONS_FROM_LISTENER => reply(self.active_from_listener(call)),
            nm::CANCEL_NOTIFICATIONS_FROM_LISTENER => empty(self.cancel_from_listener(call)),
            code => reply(Err(self.host.unimplemented("notification", code))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::parcels::StatusBarNotification;
    use crate::notifications::parcels::tests::java_notification;

    const INTENT: Binder = Binder::Local(0x10);
    const FSI: Binder = Binder::Local(0x20);

    /// A notification's parcel as the app wrote it, and what it reads as.
    fn read(raw: &Raw) -> (Raw, parcels::Notification) {
        let p = raw.parcel();
        let mut r = p.reader();
        let mut n = parcels::Notification::default();
        parcels::read_notification(&mut r, &NoFiles, &mut n).unwrap();
        assert!(n.complete);
        assert_eq!(r.remaining(), 0);
        (Raw::capture(&r, 0), n)
    }

    fn builder_notification() -> (Raw, parcels::Notification) {
        let mut p = Parcel::new();
        java_notification(&mut p, INTENT, false);
        let mut r = p.reader();
        r.set_position(p.data().len());
        read(&Raw::capture(&r, 0))
    }

    fn kept(raw: Raw, token_at: usize, flags_at: usize) -> Arc<Kept> {
        Arc::new(Kept {
            raw,
            token_at,
            flags_at,
            _binders: Arc::default(),
        })
    }

    /// A foreground service's notification is flagged in its parcel; one
    /// with actions shows at once unless the app forces its deferral.
    #[test]
    fn a_foreground_service_notification_is_flagged_and_shown_as_the_original_shows_it() {
        let (raw, n) = builder_notification();
        let incoming = Incoming {
            n,
            raw,
            _binders: Arc::default(),
        };
        let fgs = incoming.foreground_service();
        let (_, flagged) = read(&fgs.raw);
        assert_eq!(flagged.flags, incoming.n.flags | FLAG_FOREGROUND_SERVICE);
        assert!(incoming.show_foreground_immediately());
        let mut deferred = incoming.clone();
        deferred.n.fgs_defer_behavior = FOREGROUND_SERVICE_DEFERRED;
        assert!(deferred.force_deferred() && !deferred.show_foreground_immediately());
        let mut plain = incoming.clone();
        plain.n.actions.clear();
        assert!(!plain.show_foreground_immediately());
        plain.n.category = Some("navigation".into());
        assert!(plain.show_foreground_immediately());
    }

    #[test]
    fn fixes_set_flags_and_remove_extras_in_place() {
        let (raw, mut n) = builder_notification();
        let fixes = Fixes {
            flags: n.flags | FLAG_CAN_COLORIZE,
            extras: vec!["android.bigText", "android.summarization"],
            sticky_hun: false,
        };
        let (raw, flags_at) = apply(raw, &mut n, &fixes).unwrap();
        assert_eq!(flags_at, n.layout.flags);
        let (_, fixed) = read(&raw);
        assert_eq!(fixed.flags, fixes.flags);
        assert_eq!(fixed.extras.big_text, None);
        assert_eq!(fixed.extras.title.as_deref(), Some("Title"));
        assert_eq!(fixed.extras.messages.len(), 1);
        assert_eq!(fixed.channel_id.as_deref(), Some("chat"));
        assert_eq!(fixed.actions.len(), 1);
    }

    /// `makeStickyHun`: the full-screen intent becomes the content intent
    /// of a notification without one, and the flags move with it.
    #[test]
    fn a_denied_full_screen_intent_becomes_the_content_intent() {
        let (raw, n) = builder_notification();
        // The builder notification with a full-screen intent instead of a
        // content intent.
        let l = &n.layout;
        let mut fsi = Parcel::new();
        fsi.write_i32(1);
        fsi.write_binder(Some(FSI));
        let mut none = Parcel::new();
        none.write_i32(0);
        let raw = raw
            .replace(l.full_screen_intent, l.full_screen_intent + 4, &fsi)
            .replace(l.content_intent, l.content_intent + 4 + BINDER_SIZE, &none);
        let (raw, mut n) = read(&raw);
        assert_eq!((n.content_intent, n.full_screen_intent), (None, Some(FSI)));
        let fixes = Fixes {
            flags: n.flags | FLAG_FSI_REQUESTED_BUT_DENIED,
            extras: Vec::new(),
            sticky_hun: true,
        };
        let (raw, flags_at) = apply(raw, &mut n, &fixes).unwrap();
        let (_, fixed) = read(&raw);
        assert_eq!(
            (fixed.content_intent, fixed.full_screen_intent),
            (Some(FSI), None)
        );
        assert_eq!(fixed.flags, fixes.flags);
        assert_eq!(flags_at, fixed.layout.flags);
    }

    /// A listener's `StatusBarNotification` carries the allowlist token
    /// and the instance id; an app's neither (`sanitizeSbn`).
    #[test]
    fn status_bar_notifications_read_as_the_bridge_reads_them() {
        let (raw, n) = builder_notification();
        let token = Binder::Local(0x30);
        let sbn = |token: Option<Binder>| Sbn {
            package: "com.example".into(),
            op_package: "com.example".into(),
            id: 5,
            tag: Some("t".into()),
            uid: 10_123,
            initial_pid: 99,
            user: 0,
            post_time: 4,
            notification: kept(raw.clone(), n.layout.token, n.layout.flags),
            token,
            instance_id: token.map(|_| 7),
        };
        for token in [Some(token), None] {
            let mut p = Parcel::new();
            sbn(token).write_to(&mut p);
            let mut r = p.reader();
            let read = StatusBarNotification::read(&mut r, &NoFiles).unwrap();
            assert_eq!(read.key(), key(0, "com.example", 5, Some("t"), 10_123));
            assert_eq!(read.notification.allowlist_token, token);
            assert_eq!(read.notification.content_intent, Some(INTENT));
            assert_eq!(read.user, Some(0));
            assert_eq!(r.remaining(), 0);
        }
    }

    /// `RateEstimator`: no rate before a post, and posts in quick
    /// succession raise it past the default limit.
    #[test]
    fn the_enqueue_rate_follows_posts() {
        let mut rate = Rate::default();
        assert_eq!(rate.rate(0), 0.0);
        for t in 0..20 {
            rate.update(t * 10);
        }
        assert!(rate.rate(200) > DEFAULT_MAX_ENQUEUE_RATE);
        assert!(rate.rate(60_000) < DEFAULT_MAX_ENQUEUE_RATE);
    }
}
