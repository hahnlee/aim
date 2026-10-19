//! `media_session`: MediaSessionService for the app host
//! (`android-16.0.0_r1`): `createSession` with the original's checks
//! (`enforcePackageName`, `handleIncomingUser`, the per-uid limit) making
//! a MediaSessionRecord (`app_host/media_session_record.rs`), each full
//! user's MediaSessionStack (the priority order, the active sessions and
//! the media button session, picked by the audio playback the
//! AudioPlayerStateMonitor of `app_host/playback.rs` reports, its
//! receiver remembered in `Settings.Secure.MEDIA_BUTTON_RECEIVER`) and the
//! global priority session; the controllers' side: the active sessions
//! (`getSessions`) and their listeners, told of each change as the
//! handler pushes it (`pushSession1Changed`), the media key event session
//! and its listeners, told as `pushAddressedPlayerChangedLocked` tells
//! them, and the `Session2` token listeners, with the original's checks
//! (`verifySessionsRequest`, `enforceMediaPermissions`).
//!
//! No `media_communication` reports a `Session2`, so the `Session2` token
//! listeners are kept until they go but never told anything. The
//! image's configuration names no session policy provider, so every
//! session's policies are none. The user-interaction usage events
//! (`reportMediaInteractionEvent`) are tracked but not recorded, as usage
//! events are not implemented (#1723). Key and volume dispatch, remote
//! volume controllers and the rest are not implemented (#1793).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

use aim_android_xml::Element;
use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, StatusCode,
};
use aim_service_aidl::android_media_session_iactivesessionslistener as active_listener;
use aim_service_aidl::android_media_session_ionmediakeyeventsessionchangedlistener as key_listener;
use aim_service_aidl::android_media_session_isessionmanager as sm;

use super::kept::{KeptBundle, RawBundle};
use super::media_session_record::{Record, Token};
use super::pending_intents::{
    INTENT_SENDER_ACTIVITY, INTENT_SENDER_BROADCAST, INTENT_SENDER_FOREGROUND_SERVICE,
    INTENT_SENDER_SERVICE,
};
use super::settings_provider::{Caller, SettingsProvider};
use super::{AppHost, PER_USER_RANGE, reply};
use crate::bundle::Value;
use crate::package::component_resolver::Info;
use crate::package::intent::{ComponentName, Intent};

const STATUS_BAR_SERVICE: &str = "android.permission.STATUS_BAR_SERVICE";
const MEDIA_CONTENT_CONTROL: &str = "android.permission.MEDIA_CONTENT_CONTROL";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const SYSTEM_UID: i32 = 1000;
/// `UserHandle.ALL`, `UserHandle.CURRENT`, and the user the app host runs.
const USER_ALL: i32 = -1;
const USER_CURRENT: i32 = -2;
const CURRENT_USER: i32 = 0;
/// `Settings.Secure.MEDIA_BUTTON_RECEIVER`.
const MEDIA_BUTTON_RECEIVER: &str = "media_button_receiver";
/// `SESSION_CREATION_LIMIT_PER_UID`.
const SESSION_CREATION_LIMIT_PER_UID: i32 = 100;
/// `AudioPlaybackConfiguration.PLAYER_TYPE_JAM_SOUNDPOOL`.
const PLAYER_TYPE_JAM_SOUNDPOOL: i32 = 3;
/// `UNKNOWN_ERROR`: what JavaBBinder answers for an exception the parcel
/// cannot carry, such as a plain `RuntimeException`.
const UNKNOWN_ERROR: StatusCode = i32::MIN;
/// `MediaButtonReceiverHolder.COMPONENT_TYPE_*`.
const COMPONENT_TYPE_INVALID: i32 = 0;
const COMPONENT_TYPE_BROADCAST: i32 = 1;
const COMPONENT_TYPE_ACTIVITY: i32 = 2;
const COMPONENT_TYPE_SERVICE: i32 = 3;
/// `PackageManager` flags `MediaButtonReceiverHolder` queries with.
const MATCH_DIRECT_BOOT_AWARE: i64 = 0x8_0000;
const MATCH_DIRECT_BOOT_UNAWARE: i64 = 0x4_0000;
const PACKAGE_MANAGER_COMMON_FLAGS: i64 = MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE;
const MATCH_DEFAULT_ONLY: i64 = 0x1_0000;
const GET_ACTIVITIES: i64 = 0x1;
const GET_RECEIVERS: i64 = 0x2;
const GET_SERVICES: i64 = 0x4;
/// `Intent.ACTION_MEDIA_BUTTON`, `FLAG_RECEIVER_FOREGROUND`.
const ACTION_MEDIA_BUTTON: &str = "android.intent.action.MEDIA_BUTTON";
const FLAG_RECEIVER_FOREGROUND: i32 = 0x1000_0000;
/// `MediaSessionRecord.THROW_FOR_ACTIVITY_MEDIA_BUTTON_RECEIVER`,
/// `THROW_FOR_INVALID_BROADCAST_RECEIVER`.
pub(super) const THROW_FOR_ACTIVITY_MEDIA_BUTTON_RECEIVER: i64 = 272_737_196;
pub(super) const THROW_FOR_INVALID_BROADCAST_RECEIVER: i64 = 270_049_379;

/// `MediaButtonReceiverHolder`, without its PendingIntent, which only the
/// media key dispatch sends (#1793): a session's media button receiver, or the
/// one a user's record remembered.
#[derive(Clone)]
pub(super) struct ReceiverHolder {
    user: i32,
    component: Option<ComponentName>,
    package: String,
    kind: i32,
}

impl ReceiverHolder {
    /// `create(userId, broadcastReceiver)`.
    pub(super) fn broadcast(user: i32, component: ComponentName) -> ReceiverHolder {
        ReceiverHolder {
            user,
            package: component.package.clone(),
            component: Some(component),
            kind: COMPONENT_TYPE_BROADCAST,
        }
    }

    /// `create(userId, pendingIntent, sessionPackageName)`: the component
    /// the PendingIntent's creator resolves its intent to, if one.
    pub(super) fn create(
        host: &AppHost,
        user: i32,
        pending_intent: Option<Binder>,
        session_package: &str,
    ) -> Result<Option<ReceiverHolder>, Exception> {
        let Some(pi) = pending_intent else {
            return Ok(None);
        };
        let (kind, creator) = host.intent_sender_kind(pi);
        // `getComponentType(pendingIntent)`.
        let kind = match kind {
            INTENT_SENDER_BROADCAST => COMPONENT_TYPE_BROADCAST,
            INTENT_SENDER_ACTIVITY => COMPONENT_TYPE_ACTIVITY,
            INTENT_SENDER_SERVICE | INTENT_SENDER_FOREGROUND_SERVICE => COMPONENT_TYPE_SERVICE,
            _ => COMPONENT_TYPE_INVALID,
        };
        // `getComponentName`.
        let flags = match kind {
            COMPONENT_TYPE_ACTIVITY => {
                Some(PACKAGE_MANAGER_COMMON_FLAGS | MATCH_DEFAULT_ONLY | GET_ACTIVITIES)
            }
            COMPONENT_TYPE_SERVICE => Some(PACKAGE_MANAGER_COMMON_FLAGS | GET_SERVICES),
            COMPONENT_TYPE_BROADCAST => Some(PACKAGE_MANAGER_COMMON_FLAGS | GET_RECEIVERS),
            _ => None,
        };
        let found = match flags {
            Some(flags) => host
                .query_intent_components_for_intent_sender(pi, flags)?
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let component = found.into_iter().find_map(|ri| {
            let info = match ri.info {
                Info::Activity(a) => a.info,
                Info::Service(s) => s.info,
                Info::Provider(p) => p.info,
            };
            match (info.item.package_name, info.item.name) {
                (Some(package), Some(class)) if Some(&package) == creator.as_ref() => {
                    Some(ComponentName { package, class })
                }
                _ => None,
            }
        });
        Ok(Some(match component {
            Some(component) => ReceiverHolder {
                user,
                package: component.package.clone(),
                component: Some(component),
                kind,
            },
            None => {
                eprintln!(
                    "app host: media_session: Unresolvable implicit intent is set, pi={pi:?}"
                );
                ReceiverHolder {
                    user,
                    component: None,
                    package: session_package.to_string(),
                    kind: COMPONENT_TYPE_INVALID,
                }
            }
        }))
    }

    /// `flattenToString`.
    fn flatten(&self) -> String {
        match &self.component {
            Some(c) => format!("{}/{},{},{}", c.package, c.class, self.user, self.kind),
            None => String::new(),
        }
    }
}

/// `MediaButtonReceiverHolder.unflattenFromString`: `component,user[,type]`;
/// none for anything else, and a bad number throws. A type left out is
/// the component's (`getComponentType(context, componentName)`), which
/// `kind` looks up.
fn unflatten_receiver(
    info: Option<&str>,
    kind: impl FnOnce(&ComponentName) -> Result<i32, Exception>,
) -> Result<Option<ReceiverHolder>, Exception> {
    let Some(info) = info.filter(|i| !i.is_empty()) else {
        return Ok(None);
    };
    let tokens: Vec<&str> = info.split(',').collect();
    if tokens.len() != 2 && tokens.len() != 3 {
        return Ok(None);
    }
    // `ComponentName.unflattenFromString`.
    let Some(sep) = tokens[0].find('/').filter(|&sep| sep + 1 < tokens[0].len()) else {
        return Ok(None);
    };
    let (package, class) = (&tokens[0][..sep], &tokens[0][sep + 1..]);
    let class = if class.starts_with('.') {
        format!("{package}{class}")
    } else {
        class.to_string()
    };
    let component = ComponentName {
        package: package.to_string(),
        class,
    };
    // `Integer.parseInt` of the user and the type throws on a bad number.
    let number = |t: &str| {
        t.parse::<i32>()
            .map_err(|_| Exception::illegal_argument(format!("For input string: \"{t}\"")))
    };
    let user = number(tokens[1])?;
    let kind = match tokens.get(2) {
        Some(t) => number(t)?,
        None => kind(&component)?,
    };
    Ok(Some(ReceiverHolder {
        user,
        package: component.package.clone(),
        component: Some(component),
        kind,
    }))
}

/// `ManagedServices.isPackageAllowed` of the notification listeners as
/// `NotificationManagerService` loads them from `notification_policy.xml`
/// (`<enabled_listeners>`, each `<service_listing approved=.. user=..>`).
fn notification_listener_allowed(policy: Option<&Element>, package: &str, user: i32) -> bool {
    let Some(policy) = policy else { return false };
    policy
        .children()
        .filter(|e| e.name == "enabled_listeners")
        .flat_map(|e| e.children())
        .filter(|e| {
            e.name == "service_listing" && e.int("user").ok().flatten().unwrap_or(0) == user
        })
        .filter_map(|e| e.string("approved").map(|a| a.into_owned()))
        .flat_map(|approved| approved.split(':').map(str::to_owned).collect::<Vec<_>>())
        .filter(|entry| !entry.is_empty())
        .any(|entry| match entry.find('/') {
            Some(sep) if sep + 1 < entry.len() => &entry[..sep] == package,
            _ => entry == package,
        })
}

/// A session of the stack, as the stack reads it.
trait Session {
    fn uid(&self) -> i32;
    fn user(&self) -> i32;
    fn is_active(&self) -> bool;
    fn check_playback_active_state(&self, expected: bool) -> bool;
}

impl Session for Record {
    fn uid(&self) -> i32 {
        self.owner_uid
    }

    fn user(&self) -> i32 {
        self.user
    }

    fn is_active(&self) -> bool {
        Record::is_active(self)
    }

    fn check_playback_active_state(&self, expected: bool) -> bool {
        Record::check_playback_active_state(self, expected)
    }
}

/// What the stack reads of the AudioPlayerStateMonitor.
trait Monitor {
    fn sorted_uids(&self) -> Vec<i32>;
    fn is_playback_active(&self, uid: i32) -> bool;
    fn clean_up(&mut self, media_button_uid: i32);
}

impl Monitor for super::playback::StateMonitor {
    fn sorted_uids(&self) -> Vec<i32> {
        super::playback::StateMonitor::sorted_uids(self)
    }

    fn is_playback_active(&self, uid: i32) -> bool {
        super::playback::StateMonitor::is_playback_active(self, uid)
    }

    fn clean_up(&mut self, media_button_uid: i32) {
        super::playback::StateMonitor::clean_up(self, media_button_uid)
    }
}

/// A change of the media button session: the old one and the new one.
type ButtonChange<S> = (Option<Arc<S>>, Option<Arc<S>>);

/// `MediaSessionStack`. No session has policies, so none ignores the
/// button session.
struct Stack<S> {
    sessions: Vec<Arc<S>>,
    media_button_session: Option<Arc<S>>,
    /// `mCachedActiveLists`, by user.
    cached_active: HashMap<i32, Vec<Arc<S>>>,
}

impl<S> Default for Stack<S> {
    fn default() -> Self {
        Stack {
            sessions: Vec::new(),
            media_button_session: None,
            cached_active: HashMap::new(),
        }
    }
}

fn same<S>(a: &Option<Arc<S>>, b: &Option<Arc<S>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

impl<S: Session> Stack<S> {
    fn contains(&self, record: &Arc<S>) -> bool {
        self.sessions.iter().any(|s| Arc::ptr_eq(s, record))
    }

    /// `addSession`: to the bottom.
    fn add(&mut self, record: Arc<S>, monitor: &mut dyn Monitor) -> Option<ButtonChange<S>> {
        self.clear_cache(record.user());
        self.sessions.push(record);
        self.update_media_button_session_if_needed(monitor)
    }

    /// `removeSession`.
    fn remove(&mut self, record: &Arc<S>) -> Option<ButtonChange<S>> {
        self.sessions.retain(|s| !Arc::ptr_eq(s, record));
        let change = match &self.media_button_session {
            Some(b) if Arc::ptr_eq(b, record) => Some(self.update_media_button_session(None)),
            _ => None,
        };
        self.clear_cache(record.user());
        change
    }

    /// `onPlaybackStateChanged`: to the top if its priority changes, and
    /// the media button session of its uid found again.
    fn on_playback_state_changed(
        &mut self,
        record: &Arc<S>,
        update_priority: bool,
        monitor: &dyn Monitor,
    ) -> Option<ButtonChange<S>> {
        if update_priority {
            self.sessions.retain(|s| !Arc::ptr_eq(s, record));
            self.sessions.insert(0, record.clone());
            self.clear_cache(record.user());
        }
        let uid = self
            .media_button_session
            .as_ref()
            .map(|b| b.uid())
            .filter(|&uid| uid == record.uid())?;
        let found = self.find_media_button_session(uid, monitor);
        (!same(&found, &self.media_button_session)).then(|| self.update_media_button_session(found))
    }

    /// `updateMediaButtonSessionIfNeeded`: the session of the uid that
    /// played last, if any.
    fn update_media_button_session_if_needed(
        &mut self,
        monitor: &mut dyn Monitor,
    ) -> Option<ButtonChange<S>> {
        for uid in monitor.sorted_uids() {
            let Some(found) = self.find_media_button_session(uid, monitor) else {
                continue;
            };
            monitor.clean_up(found.uid());
            let found = Some(found);
            return (!same(&found, &self.media_button_session))
                .then(|| self.update_media_button_session(found));
        }
        None
    }

    /// `findMediaButtonSession`: the uid's session whose playback state
    /// agrees with its audio, else its first.
    fn find_media_button_session(&self, uid: i32, monitor: &dyn Monitor) -> Option<Arc<S>> {
        let mut first = None;
        for session in self.sessions.iter().filter(|s| s.uid() == uid) {
            if session.check_playback_active_state(monitor.is_playback_active(uid)) {
                return Some(session.clone());
            }
            first.get_or_insert_with(|| session.clone());
        }
        first
    }

    fn update_media_button_session(&mut self, new: Option<Arc<S>>) -> ButtonChange<S> {
        let old = std::mem::replace(&mut self.media_button_session, new.clone());
        (old, new)
    }

    /// `getActiveSessions`.
    fn active_sessions(&mut self, user: i32) -> Vec<Arc<S>> {
        if let Some(cached) = self.cached_active.get(&user) {
            return cached.clone();
        }
        let list = self.priority_list(true, user);
        self.cached_active.insert(user, list.clone());
        list
    }

    /// `getPriorityList`: the playing active sessions, the other active
    /// ones, then (unless `active_only`) the inactive ones, each in stack
    /// order.
    fn priority_list(&self, active_only: bool, user: i32) -> Vec<Arc<S>> {
        let mut result = Vec::new();
        let (mut last_playback_active, mut last_active) = (0, 0);
        for session in self
            .sessions
            .iter()
            .filter(|s| user == USER_ALL || s.user() == user)
        {
            if !session.is_active() {
                if !active_only {
                    result.push(session.clone());
                }
                continue;
            }
            if session.check_playback_active_state(true) {
                result.insert(last_playback_active, session.clone());
                last_playback_active += 1;
                last_active += 1;
            } else {
                result.insert(last_active, session.clone());
                last_active += 1;
            }
        }
        result
    }

    fn clear_cache(&mut self, user: i32) {
        self.cached_active.remove(&user);
        self.cached_active.remove(&USER_ALL);
    }
}

/// `FullUserRecord`, its sessions' side.
struct User {
    stack: Stack<Record>,
    /// `mUidToSessionCount`.
    session_counts: HashMap<i32, i32>,
    /// `mLastMediaButtonReceiverHolder`.
    last_receiver: Option<ReceiverHolder>,
}

/// What the service keeps under its lock (`mLock`).
#[derive(Default)]
struct Sessions {
    /// `mUserRecords`, made when first asked.
    users: BTreeMap<i32, User>,
    global_priority: Option<Arc<Record>>,
    /// `mUserEngagedSessionsForUsageLogging`, by uid.
    engaged_for_usage: HashMap<i32, Vec<Arc<Record>>>,
}

/// What a change leaves to do once the lock is let go.
enum Effect {
    /// `postSessionsChanged` of the user.
    SessionsChanged(i32),
    /// `Settings.Secure.putString(MEDIA_BUTTON_RECEIVER)` of the user.
    Remember(i32, String),
    /// `onMediaKeyEventSessionChanged` to the user's listeners.
    Addressed(i32, String, Option<Token>),
}

/// A listener and its death link.
struct Kept {
    strong: Strong,
    death: Option<u64>,
    /// The user it listens to.
    user: i32,
}

#[derive(Default)]
struct Listeners {
    /// `mSessionsListeners`, by binder.
    sessions: HashMap<u32, Kept>,
    /// Each full user's `mOnMediaKeyEventSessionChangedListeners`.
    key_event_session: HashMap<(i32, u32), Kept>,
    /// `mSession2TokensListenerRecords`, kept without a death link as the
    /// original adds them.
    session2_tokens: HashMap<u32, Kept>,
}

type Job = Box<dyn FnOnce() + Send>;

pub struct MediaSession {
    host: Arc<AppHost>,
    settings: Arc<SettingsProvider>,
    me: Weak<MediaSession>,
    listeners: Arc<Mutex<Listeners>>,
    sessions: Mutex<Sessions>,
    /// The handler's thread, and each user's latest `postSessionsChanged`
    /// (an earlier one still queued is removed, `removeMessages`).
    handler: Mutex<mpsc::Sender<Job>>,
    sessions_changed: Mutex<HashMap<i32, u64>>,
}

impl MediaSession {
    pub fn new(host: Arc<AppHost>, settings: Arc<SettingsProvider>) -> Arc<Self> {
        let (send, receive) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("media_session".into())
            .spawn(move || {
                for job in receive {
                    job();
                }
            })
            .expect("the media_session handler thread");
        let service = Arc::new_cyclic(|me| MediaSession {
            host: host.clone(),
            settings,
            me: me.clone(),
            listeners: Default::default(),
            sessions: Mutex::default(),
            handler: Mutex::new(send),
            sessions_changed: Mutex::default(),
        });
        // `onStart`'s AudioPlayerStateMonitor listener: the user's media
        // button session updated on the handler.
        let me = Arc::downgrade(&service);
        host.playback
            .lock()
            .unwrap()
            .monitor
            .set_listener(Box::new(move |uid, player_type| {
                if player_type == PLAYER_TYPE_JAM_SOUNDPOOL {
                    return;
                }
                let me = me.clone();
                if let Some(service) = me.upgrade() {
                    service.post(Box::new(move || {
                        if let Some(service) = me.upgrade() {
                            service.on_audio_player_changed(uid / PER_USER_RANGE as i32);
                        }
                    }));
                }
            }));
        service
    }

    /// Runs `job` on the handler's thread.
    pub(super) fn post(&self, job: Job) {
        let _ = self.handler.lock().unwrap().send(job);
    }

    fn held(&self, permission: &str, uid: i32) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), uid)? == 0)
    }

    /// `MediaServerUtils.enforcePackageName`.
    pub(super) fn enforce_package_name(
        &self,
        package: Option<&str>,
        uid: i32,
    ) -> Result<(), Exception> {
        if matches!(uid, 0 | 2000) {
            return Ok(());
        }
        let package = package
            .filter(|p| !p.is_empty())
            .ok_or_else(|| Exception::illegal_argument("packageName may not be empty"))?;
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::illegal_argument(format!(
                "packageName does not belong to the calling uid; pkg={package}, uid={uid}"
            )));
        }
        Ok(())
    }

    /// `handleIncomingUser`.
    fn incoming_user(&self, uid: i32, user: i32, package: Option<&str>) -> Result<i32, Exception> {
        let calling = uid / PER_USER_RANGE as i32;
        if user == calling {
            return Ok(user);
        }
        if self.held(INTERACT_ACROSS_USERS_FULL, uid)? {
            return Ok(if user == USER_CURRENT {
                CURRENT_USER
            } else {
                user
            });
        }
        Err(Exception::security(format!(
            "Permission denied while calling from {} with user id: {user}; Need to run as either the calling user id ({calling}), or with {INTERACT_ACROSS_USERS_FULL} permission",
            package.unwrap_or("null")
        )))
    }

    /// `enforceMediaPermissions`.
    fn enforce_media_permissions(
        &self,
        package: Option<&str>,
        uid: i32,
        user: i32,
    ) -> Result<(), Exception> {
        if self.held(STATUS_BAR_SERVICE, uid)?
            || uid == SYSTEM_UID
            || self.held(MEDIA_CONTENT_CONTROL, uid)?
        {
            return Ok(());
        }
        // `hasEnabledNotificationListener`: only for the caller's own user.
        let allowed = match package {
            Some(package) if uid / PER_USER_RANGE as i32 == user => {
                let path = self.host.data()?.join("system/notification_policy.xml");
                let state = |e: String| {
                    Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()))
                };
                let policy = match super::device_policy::read_atomic_bytes(&path).map_err(state)? {
                    Some(bytes) => aim_android_xml::read_next_optional(&bytes).map_err(state)?,
                    None => None,
                };
                notification_listener_allowed(policy.as_ref(), package, user)
            }
            _ => false,
        };
        if !allowed {
            return Err(Exception::security("Missing permission to control media."));
        }
        Ok(())
    }

    /// `verifySessionsRequest`.
    fn verify_sessions_request(
        &self,
        component: Option<&ComponentName>,
        user: i32,
        uid: i32,
    ) -> Result<i32, Exception> {
        let package = component.map(|c| c.package.as_str());
        if package.is_some() {
            self.enforce_package_name(package, uid)?;
        }
        let user = self.incoming_user(uid, user, package)?;
        self.enforce_media_permissions(package, uid, user)?;
        Ok(user)
    }

    /// `hasMediaControlPermission`.
    fn has_media_control_permission(&self, uid: i32) -> Result<bool, Exception> {
        Ok(uid == SYSTEM_UID || self.held(MEDIA_CONTENT_CONTROL, uid)?)
    }

    /// `getFullUserRecordLocked`'s user: the app host's users have no
    /// profiles, so each is its own full user; its record made when first
    /// asked, as `updateUser` makes it, the remembered receiver restored.
    fn full_user(&self, user: i32) -> Result<Option<i32>, Exception> {
        let (state, _) = self.host.packages()?;
        if !state.users.contains_key(&user) {
            return Ok(None);
        }
        if self.sessions.lock().unwrap().users.contains_key(&user) {
            return Ok(Some(user));
        }
        let info = self.setting(user)?;
        let last_receiver = unflatten_receiver(info.as_deref(), |c| self.component_type(c, user))?;
        self.sessions
            .lock()
            .unwrap()
            .users
            .entry(user)
            .or_insert(User {
                stack: Stack::default(),
                session_counts: HashMap::new(),
                last_receiver,
            });
        Ok(Some(user))
    }

    /// `Settings.Secure.getString(MEDIA_BUTTON_RECEIVER)` of `user`.
    fn setting(&self, user: i32) -> Result<Option<String>, Exception> {
        let caller = Caller {
            uid: SYSTEM_UID,
            package: Some("android".into()),
        };
        let extras = HashMap::from([("_user".to_string(), Value::Int(user))]);
        let got = self.settings.call(
            &caller,
            "GET_secure",
            Some(MEDIA_BUTTON_RECEIVER),
            &extras,
            None,
        )?;
        Ok(match got.as_deref() {
            Some([(_, Value::String(value)), ..]) => value.clone(),
            _ => None,
        })
    }

    /// `getComponentType(context, componentName)`: an activity, a service,
    /// else a receiver.
    fn component_type(&self, component: &ComponentName, user: i32) -> Result<i32, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let modelled = |e: crate::package::apps_filter::NotModelled| {
            self.host.unimplemented_named("media_session", e.0)
        };
        if let Ok(Some(_)) = query
            .activity_info(
                component,
                PACKAGE_MANAGER_COMMON_FLAGS | GET_ACTIVITIES,
                SYSTEM_UID,
                user,
            )
            .map_err(modelled)?
        {
            return Ok(COMPONENT_TYPE_ACTIVITY);
        }
        if let Ok(Some(_)) = query
            .service_info(component, PACKAGE_MANAGER_COMMON_FLAGS | GET_SERVICES, user)
            .map_err(modelled)?
        {
            return Ok(COMPONENT_TYPE_SERVICE);
        }
        Ok(COMPONENT_TYPE_BROADCAST)
    }

    /// `componentNameExists`: a receiver of `ACTION_MEDIA_BUTTON` the
    /// component names, none naming any. A receiver found with none named
    /// is the original's `NullPointerException` making the holder.
    pub(super) fn media_button_receiver_exists(
        &self,
        component: Option<&ComponentName>,
        user: i32,
    ) -> Result<bool, Exception> {
        let intent = Intent {
            action: Some(ACTION_MEDIA_BUTTON.into()),
            flags: FLAG_RECEIVER_FOREGROUND,
            component: component.cloned(),
            ..Default::default()
        };
        let (_, resolution) = self.host.packages()?;
        let found = resolution
            .query_intent_receivers(&intent, None, 0, user, SYSTEM_UID)
            .map_err(|e| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("queryBroadcastReceiversAsUser: {e:?}"),
                )
            })?;
        if component.is_none() && !found.is_empty() {
            return Err(Exception::new(
                EX_NULL_POINTER,
                "Attempt to invoke virtual method 'java.lang.String android.content.ComponentName.getPackageName()' on a null object reference",
            ));
        }
        Ok(!found.is_empty())
    }

    /// `createSessionInternal`.
    fn create_session(
        &self,
        uid: i32,
        user: i32,
        package: String,
        cb: u32,
        tag: Option<String>,
        session_info: Option<RawBundle>,
    ) -> Result<Arc<Record>, Result<Exception, String>> {
        let user_record = self.full_user(user).map_err(Ok)?;
        if user_record.is_none() {
            eprintln!("app host: media_session: Request from invalid user: {user}, pkg={package}");
            return Err(Err("Session request from invalid user.".into()));
        }
        let count = self
            .sessions
            .lock()
            .unwrap()
            .users
            .get(&user)
            .map_or(0, |u| u.session_counts.get(&uid).copied().unwrap_or(0));
        if count >= SESSION_CREATION_LIMIT_PER_UID
            && !self.has_media_control_permission(uid).map_err(Ok)?
        {
            return Err(Err(format!("Created too many sessions. count={count})")));
        }
        let session_info = session_info
            .map(|i| KeptBundle::new(i, &self.host.process))
            .transpose()
            .map_err(|_| Ok(Exception::illegal_argument("a parcel that does not read")))?;
        let me = self
            .me
            .upgrade()
            .ok_or_else(|| Ok(Exception::new(EX_ILLEGAL_STATE, "media_session gone")))?;
        let record = Record::new(&me, &self.host, (uid, user), package, cb, tag, session_info);
        let mut effects = Vec::new();
        {
            let mut s = self.sessions.lock().unwrap();
            let u = s.users.get_mut(&user).expect("made above");
            *u.session_counts.entry(uid).or_insert(0) += 1;
            let change = u.stack.add(
                record.clone(),
                &mut self.host.playback.lock().unwrap().monitor,
            );
            self.media_button_session_changed(&mut s, user, change, &mut effects);
        }
        effects.push(Effect::SessionsChanged(user));
        self.apply(effects);
        Ok(record)
    }

    /// `FullUserRecord.onMediaButtonSessionChanged`.
    fn media_button_session_changed(
        &self,
        s: &mut Sessions,
        user: i32,
        change: Option<ButtonChange<Record>>,
        effects: &mut Vec<Effect>,
    ) {
        let Some((old, new)) = change else { return };
        if let Some(old) = old {
            effects.push(Effect::SessionsChanged(old.user));
        }
        if let Some(new) = new {
            self.remember_media_button_receiver(s, user, &new, effects);
            effects.push(Effect::SessionsChanged(new.user));
        }
        effects.push(self.addressed(s, user));
    }

    /// `rememberMediaButtonReceiverLocked`.
    fn remember_media_button_receiver(
        &self,
        s: &mut Sessions,
        user: i32,
        record: &Record,
        effects: &mut Vec<Effect>,
    ) {
        let Some(u) = s.users.get_mut(&user) else {
            return;
        };
        u.last_receiver = record.media_button_receiver();
        effects.push(Effect::Remember(
            user,
            u.last_receiver
                .as_ref()
                .map_or_else(String::new, ReceiverHolder::flatten),
        ));
    }

    /// `getMediaButtonSessionLocked`.
    fn media_button_session(s: &Sessions, user: i32) -> Option<Arc<Record>> {
        if Self::is_global_priority_active(s) {
            return s.global_priority.clone();
        }
        s.users
            .get(&user)
            .and_then(|u| u.stack.media_button_session.clone())
    }

    fn is_global_priority_active(s: &Sessions) -> bool {
        s.global_priority.as_ref().is_some_and(|g| g.is_active())
    }

    /// `pushAddressedPlayerChangedLocked`'s arguments: the media button
    /// session, else the current user's last receiver's package, else
    /// none.
    fn addressed(&self, s: &Sessions, user: i32) -> Effect {
        if let Some(session) = Self::media_button_session(s, user) {
            return Effect::Addressed(user, session.package.clone(), Some(session.token()));
        }
        match s
            .users
            .get(&CURRENT_USER)
            .and_then(|u| u.last_receiver.as_ref())
        {
            Some(holder) => Effect::Addressed(user, holder.package.clone(), None),
            None => Effect::Addressed(user, String::new(), None),
        }
    }

    /// Does what the changes left, the lock let go.
    fn apply(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::SessionsChanged(user) => self.post_sessions_changed(user),
                Effect::Remember(user, info) => {
                    let caller = Caller {
                        uid: SYSTEM_UID,
                        package: Some("android".into()),
                    };
                    let extras = HashMap::from([
                        ("_user".to_string(), Value::Int(user)),
                        ("value".to_string(), Value::String(Some(info))),
                    ]);
                    if let Err(e) = self.settings.call(
                        &caller,
                        "PUT_secure",
                        Some(MEDIA_BUTTON_RECEIVER),
                        &extras,
                        None,
                    ) {
                        eprintln!(
                            "app host: media_session: {MEDIA_BUTTON_RECEIVER}: {}",
                            e.message
                        );
                    }
                }
                Effect::Addressed(user, package, token) => {
                    let handles: Vec<u32> = self
                        .listeners
                        .lock()
                        .unwrap()
                        .key_event_session
                        .keys()
                        .filter(|(u, _)| *u == user)
                        .map(|(_, h)| *h)
                        .collect();
                    let mut p = Parcel::new();
                    key_listener::OnMediaKeyEventSessionChanged {
                        package_name: Some(package),
                        media_key_event_session_token: token,
                    }
                    .write(&mut p);
                    for handle in handles {
                        if let Err(e) = self.host.process.transact(
                            handle,
                            key_listener::ON_MEDIA_KEY_EVENT_SESSION_CHANGED,
                            &p,
                            true,
                        ) {
                            eprintln!(
                                "app host: media_session: Failed to pushAddressedPlayerChangedLocked: {e}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// `MessageHandler.postSessionsChanged`: `pushSession1Changed` of the
    /// user on the handler, an earlier one still queued removed.
    fn post_sessions_changed(&self, user: i32) {
        let generation = {
            let mut changed = self.sessions_changed.lock().unwrap();
            let generation = changed.entry(user).or_insert(0);
            *generation += 1;
            *generation
        };
        let me = self.me.clone();
        self.post(Box::new(move || {
            if let Some(service) = me.upgrade()
                && service.sessions_changed.lock().unwrap().get(&user) == Some(&generation)
            {
                service.push_session1_changed(user);
            }
        }));
    }

    /// `pushSession1Changed`: the user's active sessions to its listeners
    /// and those of all users, the dead ones dropped.
    fn push_session1_changed(&self, user: i32) {
        let tokens = {
            let mut s = self.sessions.lock().unwrap();
            if !s.users.contains_key(&user) {
                eprintln!(
                    "app host: media_session: pushSession1ChangedOnHandler failed. No user with id={user}"
                );
                return;
            }
            Self::active_sessions(&mut s, user)
                .iter()
                .map(|r| Some(r.token()))
                .collect::<Vec<_>>()
        };
        let handles: Vec<u32> = self
            .listeners
            .lock()
            .unwrap()
            .sessions
            .iter()
            .filter(|(_, k)| k.user == USER_ALL || k.user == user)
            .map(|(h, _)| *h)
            .collect();
        let mut p = Parcel::new();
        active_listener::OnActiveSessionsChanged {
            sessions: Some(tokens),
        }
        .write(&mut p);
        for handle in handles {
            if let Err(e) = self.host.process.transact(
                handle,
                active_listener::ON_ACTIVE_SESSIONS_CHANGED,
                &p,
                true,
            ) {
                eprintln!(
                    "app host: media_session: Dead ActiveSessionsListener in pushSessionsChanged, removing: {e}"
                );
                self.forget(&handle, sessions);
            }
        }
    }

    /// `getActiveSessionsLocked`: the global priority session first if it
    /// is active and the user's.
    fn active_sessions(s: &mut Sessions, user: i32) -> Vec<Arc<Record>> {
        let mut records = Vec::new();
        if user == USER_ALL {
            for u in s.users.values_mut() {
                records.extend(u.stack.active_sessions(user));
            }
        } else if let Some(u) = s.users.get_mut(&user) {
            records.extend(u.stack.active_sessions(user));
        } else {
            eprintln!("app host: media_session: getSessions failed. Unknown user {user}");
            return records;
        }
        if Self::is_global_priority_active(s)
            && let Some(global) = &s.global_priority
            && (user == USER_ALL || user == global.user)
        {
            records.insert(0, global.clone());
        }
        records
    }

    /// The audio playback changed: `updateMediaButtonSessionIfNeeded` of
    /// the uid's user.
    fn on_audio_player_changed(&self, user: i32) {
        let mut effects = Vec::new();
        {
            let mut s = self.sessions.lock().unwrap();
            let Some(u) = s.users.get_mut(&user) else {
                return;
            };
            let change = u.stack.update_media_button_session_if_needed(
                &mut self.host.playback.lock().unwrap().monitor,
            );
            self.media_button_session_changed(&mut s, user, change, &mut effects);
        }
        self.apply(effects);
    }

    /// `onSessionActiveStateChanged`.
    pub(super) fn on_session_active_state_changed(&self, record: &Arc<Record>) {
        let mut effects = Vec::new();
        {
            let mut s = self.sessions.lock().unwrap();
            let user = record.user;
            if !s.users.contains_key(&user) {
                eprintln!("app host: media_session: Unknown session updated. Ignoring.");
                return;
            }
            if record.is_system_priority() {
                effects.push(self.addressed(&s, user));
            } else {
                let u = s.users.get_mut(&user).unwrap();
                if !u.stack.contains(record) {
                    eprintln!("app host: media_session: Unknown session updated. Ignoring.");
                    return;
                }
                u.stack.clear_cache(user);
            }
            let engaged = record.is_user_engaged();
            self.report_media_interaction_event(&mut s, record, engaged);
            effects.push(Effect::SessionsChanged(user));
        }
        self.apply(effects);
    }

    /// `setGlobalPrioritySession`.
    pub(super) fn set_global_priority_session(&self, record: &Arc<Record>) {
        let mut s = self.sessions.lock().unwrap();
        if s.global_priority
            .as_ref()
            .is_some_and(|g| Arc::ptr_eq(g, record))
        {
            return;
        }
        s.global_priority = Some(record.clone());
        if let Some(u) = s.users.get_mut(&record.user)
            && u.stack.contains(record)
        {
            // A media button session removed this way is not announced, as
            // the original's `removeSession` here is not.
            let _ = u.stack.remove(record);
        }
    }

    /// `onSessionPlaybackStateChanged`.
    pub(super) fn on_session_playback_state_changed(
        &self,
        record: &Arc<Record>,
        update_priority: bool,
    ) {
        let mut effects = Vec::new();
        {
            let mut s = self.sessions.lock().unwrap();
            let user = record.user;
            let Some(u) = s.users.get_mut(&user).filter(|u| u.stack.contains(record)) else {
                return;
            };
            let change = u.stack.on_playback_state_changed(
                record,
                update_priority,
                &self.host.playback.lock().unwrap().monitor,
            );
            self.media_button_session_changed(&mut s, user, change, &mut effects);
            let engaged = record.is_user_engaged();
            self.report_media_interaction_event(&mut s, record, engaged);
        }
        self.apply(effects);
    }

    /// `onSessionPlaybackTypeChanged`: `pushRemoteVolumeUpdateLocked`,
    /// which tells the remote volume controllers, of which none can
    /// register (#1793).
    pub(super) fn on_session_playback_type_changed(&self, _: &Arc<Record>) {}

    /// `onMediaButtonReceiverChanged`: remembered if it is the media
    /// button session's.
    pub(super) fn on_media_button_receiver_changed(&self, record: &Arc<Record>) {
        let mut effects = Vec::new();
        {
            let mut s = self.sessions.lock().unwrap();
            let user = record.user;
            let is_button_session = s
                .users
                .get(&user)
                .and_then(|u| u.stack.media_button_session.as_ref())
                .is_some_and(|b| Arc::ptr_eq(b, record));
            if is_button_session {
                self.remember_media_button_receiver(&mut s, user, record, &mut effects);
            }
        }
        self.apply(effects);
    }

    /// `onSessionDied`: `destroySessionLocked`.
    pub(super) fn on_session_died(&self, record: &Arc<Record>) {
        if record.is_closed() {
            eprintln!("app host: media_session: Destroying already destroyed session. Ignoring.");
            return;
        }
        let mut effects = Vec::new();
        {
            let mut s = self.sessions.lock().unwrap();
            let user = record.user;
            if let Some(u) = s.users.get_mut(&user) {
                let count = u.session_counts.entry(record.owner_uid).or_insert(0);
                if *count <= 0 {
                    eprintln!(
                        "app host: media_session: destroySessionLocked: sessionCount should be positive. sessionCount={count}"
                    );
                } else {
                    *count -= 1;
                }
            }
            if s.global_priority
                .as_ref()
                .is_some_and(|g| Arc::ptr_eq(g, record))
            {
                s.global_priority = None;
                if record.is_active() && s.users.contains_key(&user) {
                    effects.push(self.addressed(&s, user));
                }
            } else if let Some(u) = s.users.get_mut(&user) {
                let change = u.stack.remove(record);
                self.media_button_session_changed(&mut s, user, change, &mut effects);
            }
            record.close();
            self.report_media_interaction_event(&mut s, record, false);
            effects.push(Effect::SessionsChanged(user));
        }
        self.apply(effects);
    }

    /// `reportMediaInteractionEvent`: the uid's engaged sessions tracked;
    /// its first and its last are the start and stop of a usage event,
    /// which is not recorded (#1723).
    fn report_media_interaction_event(
        &self,
        s: &mut Sessions,
        record: &Arc<Record>,
        engaged: bool,
    ) {
        let uid = record.owner_uid;
        let action = if engaged {
            let records = s.engaged_for_usage.entry(uid).or_default();
            let first = records.is_empty();
            if !records.iter().any(|r| Arc::ptr_eq(r, record)) {
                records.push(record.clone());
            }
            first.then_some("start")
        } else {
            let Some(records) = s.engaged_for_usage.get_mut(&uid) else {
                return;
            };
            records.retain(|r| !Arc::ptr_eq(r, record));
            if !records.is_empty() {
                return;
            }
            s.engaged_for_usage.remove(&uid);
            Some("stop")
        };
        if let Some(action) = action {
            eprintln!(
                "app host: media_session: reportUserInteractionEvent(android.media, {action}) of {} not recorded (#1723)",
                record.package
            );
        }
    }

    /// Keeps `listener` under `key` in the map `pick` chooses, linked to its
    /// death unless `link` is off.
    fn keep<K: Copy + Eq + std::hash::Hash + Send + 'static>(
        &self,
        handle: u32,
        key: K,
        user: i32,
        link: bool,
        pick: fn(&mut Listeners) -> &mut HashMap<K, Kept>,
    ) {
        let strong = self.host.process.strong(handle);
        let death = link.then(|| {
            let listeners = Arc::downgrade(&self.listeners);
            let process = self.host.process.clone();
            self.host.process.link_to_death(
                &strong,
                Box::new(move || {
                    if let Some(listeners) = listeners.upgrade()
                        && let Some(kept) = pick(&mut listeners.lock().unwrap()).remove(&key)
                        && let Some(death) = kept.death
                    {
                        process.clear_death(&kept.strong, death);
                    }
                }),
            )
        });
        pick(&mut self.listeners.lock().unwrap()).insert(
            key,
            Kept {
                strong,
                death,
                user,
            },
        );
    }

    fn forget<K: Eq + std::hash::Hash>(
        &self,
        key: &K,
        pick: fn(&mut Listeners) -> &mut HashMap<K, Kept>,
    ) {
        let kept = pick(&mut self.listeners.lock().unwrap()).remove(key);
        if let Some(kept) = kept
            && let Some(death) = kept.death
        {
            self.host.process.clear_death(&kept.strong, death);
        }
    }

    /// `getMediaKeyEventSession`'s and `getMediaKeyEventSessionPackageName`'s
    /// checks, then the user's media button session and last receiver.
    fn key_event_session(
        &self,
        uid: i32,
        package: Option<&str>,
    ) -> Result<Option<(Option<Arc<Record>>, Option<String>)>, Exception> {
        let user = uid / PER_USER_RANGE as i32;
        self.enforce_package_name(package, uid)?;
        self.enforce_media_permissions(package, uid, user)?;
        if self.full_user(user)?.is_none() {
            eprintln!(
                "app host: media_session: No matching user record to get the media key event session, userId={user}"
            );
            return Ok(None);
        }
        let s = self.sessions.lock().unwrap();
        let last = s
            .users
            .get(&user)
            .and_then(|u| u.last_receiver.as_ref())
            .map(|h| h.package.clone());
        Ok(Some((Self::media_button_session(&s, user), last)))
    }
}

fn sessions(l: &mut Listeners) -> &mut HashMap<u32, Kept> {
    &mut l.sessions
}

fn key_event_session(l: &mut Listeners) -> &mut HashMap<(i32, u32), Kept> {
    &mut l.key_event_session
}

fn session2_tokens(l: &mut Listeners) -> &mut HashMap<u32, Kept> {
    &mut l.session2_tokens
}

impl Service for MediaSession {
    fn descriptor(&self) -> &str {
        sm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            sm::CREATE_SESSION => {
                let a =
                    sm::CreateSession::<RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let created = (|| {
                    self.enforce_package_name(a.package_name.as_deref(), uid)
                        .map_err(Ok)?;
                    let user = self
                        .incoming_user(uid, a.user_id, a.package_name.as_deref())
                        .map_err(Ok)?;
                    let Some(Binder::Handle(cb)) = a.session_cb else {
                        return Err(Ok(Exception::illegal_argument(
                            "Controller callback cannot be null",
                        )));
                    };
                    let package = a.package_name.clone().unwrap_or_default();
                    self.create_session(uid, user, package, cb, a.tag, a.session_info)
                })();
                match created {
                    Ok(record) => {
                        sm::write_create_session_reply(&mut p, Some(record.session_binder()));
                        Ok(())
                    }
                    Err(Ok(e)) => {
                        eprintln!(
                            "app host: media_session: Exception in creating a new session: {}",
                            e.message
                        );
                        Err(e)
                    }
                    Err(Err(message)) => {
                        eprintln!(
                            "app host: media_session: Exception in creating a new session: java.lang.RuntimeException: {message}"
                        );
                        return Err(UNKNOWN_ERROR);
                    }
                }
            }
            sm::GET_SESSIONS => {
                let a = sm::GetSessions::<ComponentName>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.verify_sessions_request(a.comp_name.as_ref(), a.user_id, uid)
                    .and_then(|user| {
                        if user != USER_ALL {
                            self.full_user(user)?;
                        }
                        let tokens: Vec<Option<Token>> =
                            Self::active_sessions(&mut self.sessions.lock().unwrap(), user)
                                .iter()
                                .map(|r| Some(r.token()))
                                .collect();
                        sm::write_get_sessions_reply(&mut p, Some(&tokens));
                        Ok(())
                    })
            }
            sm::ADD_SESSIONS_LISTENER => {
                let a = sm::AddSessionsListener::<ComponentName>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let Some(Binder::Handle(handle)) = a.listener else {
                        eprintln!("app host: media_session: addSessionsListener: listener is null, ignoring");
                        return Ok(());
                    };
                    let user = self.verify_sessions_request(a.comp_name.as_ref(), a.user_id, uid)?;
                    if self.listeners.lock().unwrap().sessions.contains_key(&handle) {
                        eprintln!("app host: media_session: ActiveSessionsListener is already added, ignoring");
                        return Ok(());
                    }
                    self.keep(handle, handle, user, true, sessions);
                    Ok(())
                })()
                .map(|()| sm::write_add_sessions_listener_reply(&mut p))
            }
            sm::REMOVE_SESSIONS_LISTENER => {
                let a = sm::RemoveSessionsListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(handle)) = a.listener {
                    self.forget(&handle, sessions);
                }
                sm::write_remove_sessions_listener_reply(&mut p);
                Ok(())
            }
            sm::GET_MEDIA_KEY_EVENT_SESSION => {
                let a = sm::GetMediaKeyEventSession::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.key_event_session(uid, a.package_name.as_deref())
                    .map(|found| {
                        let token = found.and_then(|(session, _)| session).map(|s| s.token());
                        sm::write_get_media_key_event_session_reply(&mut p, token.as_ref());
                    })
            }
            sm::GET_MEDIA_KEY_EVENT_SESSION_PACKAGE_NAME => {
                let a = sm::GetMediaKeyEventSessionPackageName::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.key_event_session(uid, a.package_name.as_deref())
                    .map(|found| {
                        let package = match found {
                            Some((Some(session), _)) => session.package.clone(),
                            Some((None, Some(last))) => last,
                            _ => String::new(),
                        };
                        sm::write_get_media_key_event_session_package_name_reply(
                            &mut p,
                            &Some(package),
                        );
                    })
            }
            sm::ADD_ON_MEDIA_KEY_EVENT_SESSION_CHANGED_LISTENER => {
                let a = sm::AddOnMediaKeyEventSessionChangedListener::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let Some(Binder::Handle(handle)) = a.listener else {
                        eprintln!("app host: media_session: addOnMediaKeyEventSessionChangedListener: listener is null, ignoring");
                        return Ok(());
                    };
                    let user = uid / PER_USER_RANGE as i32;
                    self.enforce_package_name(a.package_name.as_deref(), uid)?;
                    self.enforce_media_permissions(a.package_name.as_deref(), uid, user)?;
                    if self.full_user(user)?.is_none() {
                        eprintln!("app host: media_session: Only the full user can add the listener, userId={user}");
                        return Ok(());
                    }
                    self.keep(handle, (user, handle), user, true, key_event_session);
                    Ok(())
                })()
                .map(|()| sm::write_add_on_media_key_event_session_changed_listener_reply(&mut p))
            }
            sm::REMOVE_ON_MEDIA_KEY_EVENT_SESSION_CHANGED_LISTENER => {
                let a = sm::RemoveOnMediaKeyEventSessionChangedListener::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let Some(Binder::Handle(handle)) = a.listener else {
                        eprintln!("app host: media_session: removeOnMediaKeyEventSessionChangedListener: listener is null, ignoring");
                        return Ok(());
                    };
                    let user = uid / PER_USER_RANGE as i32;
                    if self.full_user(user)?.is_none() {
                        eprintln!("app host: media_session: Only the full user can remove the listener, userId={user}");
                        return Ok(());
                    }
                    self.forget(&(user, handle), key_event_session);
                    Ok(())
                })()
                .map(|()| sm::write_remove_on_media_key_event_session_changed_listener_reply(&mut p))
            }
            sm::ADD_SESSION2TOKENS_LISTENER => {
                let a =
                    sm::AddSession2TokensListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let Some(Binder::Handle(handle)) = a.listener else {
                        eprintln!("app host: media_session: addSession2TokensListener: listener is null, ignoring");
                        return Ok(());
                    };
                    let user = self.incoming_user(uid, a.user_id, None)?;
                    if self.listeners.lock().unwrap().session2_tokens.contains_key(&handle) {
                        eprintln!("app host: media_session: addSession2TokensListener: listener is already added, ignoring");
                        return Ok(());
                    }
                    self.keep(handle, handle, user, false, session2_tokens);
                    Ok(())
                })()
                .map(|()| sm::write_add_session2tokens_listener_reply(&mut p))
            }
            sm::REMOVE_SESSION2TOKENS_LISTENER => {
                let a = sm::RemoveSession2TokensListener::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(handle)) = a.listener {
                    self.forget(&handle, session2_tokens);
                }
                sm::write_remove_session2tokens_listener_reply(&mut p);
                Ok(())
            }
            sm::IS_GLOBAL_PRIORITY_ACTIVE => {
                sm::IsGlobalPriorityActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let active = Self::is_global_priority_active(&self.sessions.lock().unwrap());
                sm::write_is_global_priority_active_reply(&mut p, active);
                Ok(())
            }
            code => {
                let name = sm::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("media_session", &format!("{name} (#1793)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// `MediaButtonReceiverHolder.unflattenFromString`: a component, a
    /// user and an optional type (else the component's); anything else is
    /// no receiver, and a bad number throws.
    #[test]
    fn media_button_receivers_unflatten_as_the_holder_does() {
        let unflatten = |s: &str| unflatten_receiver(Some(s), |_| Ok(COMPONENT_TYPE_ACTIVITY));
        let r = unflatten("com.example/.Receiver,0").unwrap().unwrap();
        assert_eq!(
            (
                r.package.as_str(),
                r.component.as_ref().unwrap().class.as_str(),
                r.kind
            ),
            (
                "com.example",
                "com.example.Receiver",
                COMPONENT_TYPE_ACTIVITY
            )
        );
        assert_eq!(r.flatten(), "com.example/com.example.Receiver,0,2");
        let r = unflatten("com.example/com.example.Service,10,3")
            .unwrap()
            .unwrap();
        assert_eq!((r.user, r.kind), (10, COMPONENT_TYPE_SERVICE));
        assert!(unflatten("com.example/.Receiver").unwrap().is_none());
        assert!(unflatten("com.example,0").unwrap().is_none());
        assert!(unflatten("a/b,0,1,2").unwrap().is_none());
        assert!(
            unflatten_receiver(Some(""), |_| unreachable!())
                .unwrap()
                .is_none()
        );
        assert!(
            unflatten_receiver(None, |_| unreachable!())
                .unwrap()
                .is_none()
        );
        assert!(unflatten("com.example/.Receiver,x").is_err());
        let broadcast = ReceiverHolder::broadcast(
            0,
            ComponentName {
                package: "p".into(),
                class: "p.R".into(),
            },
        );
        assert_eq!(broadcast.flatten(), "p/p.R,0,1");
    }

    /// `isPackageAllowed`: an approved component's package or an approved
    /// package, of the user's listings only.
    #[test]
    fn notification_listeners_are_allowed_by_package() {
        let xml = br#"<notification-policy><enabled_listeners version="4"><service_listing approved="com.a/com.a.Listener:com.b" user="0" primary="true" /><service_listing approved="com.c/.L" user="10" primary="true" /></enabled_listeners></notification-policy>"#;
        let policy = aim_android_xml::read(xml).unwrap();
        let allowed = |p: &str, user| notification_listener_allowed(Some(&policy), p, user);
        assert!(allowed("com.a", 0));
        assert!(allowed("com.b", 0));
        assert!(!allowed("com.c", 0));
        assert!(allowed("com.c", 10));
        assert!(!allowed("com.d", 0));
        assert!(!notification_listener_allowed(None, "com.a", 0));
    }

    struct Fake {
        uid: i32,
        active: AtomicBool,
        playing: AtomicBool,
    }

    impl Session for Fake {
        fn uid(&self) -> i32 {
            self.uid
        }

        fn user(&self) -> i32 {
            0
        }

        fn is_active(&self) -> bool {
            self.active.load(Ordering::Relaxed)
        }

        fn check_playback_active_state(&self, expected: bool) -> bool {
            self.playing.load(Ordering::Relaxed) == expected
        }
    }

    #[derive(Default)]
    struct FakeMonitor {
        sorted: Vec<i32>,
        active: Vec<i32>,
        cleaned: Vec<i32>,
    }

    impl Monitor for FakeMonitor {
        fn sorted_uids(&self) -> Vec<i32> {
            self.sorted.clone()
        }

        fn is_playback_active(&self, uid: i32) -> bool {
            self.active.contains(&uid)
        }

        fn clean_up(&mut self, media_button_uid: i32) {
            self.cleaned.push(media_button_uid);
        }
    }

    fn fake(uid: i32, active: bool, playing: bool) -> Arc<Fake> {
        Arc::new(Fake {
            uid,
            active: AtomicBool::new(active),
            playing: AtomicBool::new(playing),
        })
    }

    fn uids(list: &[Arc<Fake>]) -> Vec<i32> {
        list.iter().map(|f| f.uid).collect()
    }

    /// `getPriorityList`: playing active sessions first, then the other
    /// active ones, then the inactive ones, each in stack order; the
    /// active list is cached until the stack changes.
    #[test]
    fn the_priority_list_puts_playing_sessions_first() {
        let mut stack = Stack::default();
        let mut monitor = FakeMonitor::default();
        let (a, b, c, d) = (
            fake(1, true, false),
            fake(2, false, false),
            fake(3, true, true),
            fake(4, true, true),
        );
        for s in [&a, &b, &c, &d] {
            assert!(stack.add(s.clone(), &mut monitor).is_none());
        }
        assert_eq!(uids(&stack.priority_list(false, USER_ALL)), [3, 4, 1, 2]);
        assert_eq!(uids(&stack.active_sessions(0)), [3, 4, 1]);
        // A playback change without a priority change keeps the cache.
        c.playing.store(false, Ordering::Relaxed);
        assert_eq!(uids(&stack.active_sessions(0)), [3, 4, 1]);
        assert!(
            stack
                .on_playback_state_changed(&a, true, &monitor)
                .is_none()
        );
        assert_eq!(uids(&stack.active_sessions(0)), [4, 1, 3]);
        assert!(stack.remove(&d).is_none());
        assert_eq!(uids(&stack.active_sessions(0)), [1, 3]);
        assert!(stack.priority_list(true, 10).is_empty());
    }

    /// `updateMediaButtonSessionIfNeeded`: the first uid of the sorted
    /// playback uids with a session; of its sessions the one whose state
    /// agrees with its audio, else its first; its uids cleaned up.
    #[test]
    fn the_media_button_session_follows_audio_playback() {
        let mut stack = Stack::default();
        let mut monitor = FakeMonitor {
            sorted: vec![7, 2],
            active: vec![2],
            cleaned: Vec::new(),
        };
        let (paused, playing) = (fake(2, true, false), fake(2, true, true));
        let (old, new) = stack.add(paused.clone(), &mut monitor).unwrap();
        assert!(old.is_none() && Arc::ptr_eq(new.as_ref().unwrap(), &paused));
        let (old, new) = stack.add(playing.clone(), &mut monitor).unwrap();
        assert!(
            Arc::ptr_eq(old.as_ref().unwrap(), &paused)
                && Arc::ptr_eq(new.as_ref().unwrap(), &playing)
        );
        assert_eq!(monitor.cleaned, [2, 2]);
        assert!(
            stack
                .update_media_button_session_if_needed(&mut monitor)
                .is_none()
        );
        // Its uid's playback stopping finds the paused one again.
        monitor.active.clear();
        let (_, new) = stack
            .on_playback_state_changed(&playing, false, &monitor)
            .unwrap();
        assert!(Arc::ptr_eq(new.as_ref().unwrap(), &paused));
        let (old, new) = stack.remove(&paused).unwrap();
        assert!(Arc::ptr_eq(old.as_ref().unwrap(), &paused) && new.is_none());
    }
}
