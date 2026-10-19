//! `media_session`: MediaSessionService for the app host
//! (`android-16.0.0_r1`), the side controllers read: the active sessions
//! (`getSessions`) and their listeners (`addSessionsListener`), the
//! media key event session (`getMediaKeyEventSession`,
//! `getMediaKeyEventSessionPackageName`, the last media button receiver
//! the user's record restores from `Settings.Secure.MEDIA_BUTTON_RECEIVER`)
//! and its listeners, the `Session2` token listeners and the global
//! priority session, with the original's checks (`verifySessionsRequest`,
//! `enforceMediaPermissions`, `handleIncomingUser`, `enforcePackageName`).
//!
//! No app can create a session (`createSession` is not implemented,
//! #1793), and no `media_communication` reports a `Session2`, so there are
//! no sessions: the lists are empty, there is no media button session and
//! no global priority session, and the listeners are kept until they go
//! but never told anything, as the original tells them only of changes.
//! Key and volume dispatch and the rest are not implemented (#1793).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_android_xml::Element;
use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_media_session_isessionmanager as sm;

use super::settings::Table;
use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::intent::ComponentName;

const STATUS_BAR_SERVICE: &str = "android.permission.STATUS_BAR_SERVICE";
const MEDIA_CONTENT_CONTROL: &str = "android.permission.MEDIA_CONTENT_CONTROL";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const SYSTEM_UID: i32 = 1000;
/// `UserHandle.CURRENT`, and the user the app host runs.
const USER_CURRENT: i32 = -2;
const CURRENT_USER: i32 = 0;
/// `Settings.Secure.MEDIA_BUTTON_RECEIVER`.
const MEDIA_BUTTON_RECEIVER: &str = "media_button_receiver";

/// A `MediaSession.Token`, of which there is none.
enum NoToken {}

impl WriteParcelable for NoToken {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

/// `MediaButtonReceiverHolder.unflattenFromString`: the package of the
/// remembered receiver, `component,user[,type]`; none for anything else.
fn media_button_receiver_package(info: Option<&str>) -> Result<Option<String>, Exception> {
    let Some(info) = info.filter(|i| !i.is_empty()) else { return Ok(None) };
    let tokens: Vec<&str> = info.split(',').collect();
    if tokens.len() != 2 && tokens.len() != 3 {
        return Ok(None);
    }
    // `ComponentName.unflattenFromString`.
    let Some(sep) = tokens[0].find('/').filter(|&sep| sep + 1 < tokens[0].len()) else { return Ok(None) };
    // `Integer.parseInt` of the user and the type throws on a bad number.
    let number = |t: &str| t.parse::<i32>().map_err(|_| Exception::illegal_argument(format!("For input string: \"{t}\"")));
    number(tokens[1])?;
    if let Some(kind) = tokens.get(2) {
        number(kind)?;
    }
    Ok(Some(tokens[0][..sep].to_string()))
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
        .filter(|e| e.name == "service_listing" && e.int("user").ok().flatten().unwrap_or(0) == user)
        .filter_map(|e| e.string("approved").map(|a| a.into_owned()))
        .flat_map(|approved| approved.split(':').map(str::to_owned).collect::<Vec<_>>())
        .filter(|entry| !entry.is_empty())
        .any(|entry| match entry.find('/') {
            Some(sep) if sep + 1 < entry.len() => &entry[..sep] == package,
            _ => entry == package,
        })
}

/// A listener and its death link.
struct Kept {
    strong: Strong,
    death: Option<u64>,
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

pub struct MediaSession {
    host: Arc<AppHost>,
    listeners: Arc<Mutex<Listeners>>,
}

impl MediaSession {
    pub fn new(host: Arc<AppHost>) -> Self {
        MediaSession { host, listeners: Default::default() }
    }

    fn held(&self, permission: &str, uid: i32) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), uid)? == 0)
    }

    /// `MediaServerUtils.enforcePackageName`.
    fn enforce_package_name(&self, package: Option<&str>, uid: i32) -> Result<(), Exception> {
        if matches!(uid, 0 | 2000) {
            return Ok(());
        }
        let package = package.filter(|p| !p.is_empty()).ok_or_else(|| Exception::illegal_argument("packageName may not be empty"))?;
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
            return Ok(if user == USER_CURRENT { CURRENT_USER } else { user });
        }
        Err(Exception::security(format!(
            "Permission denied while calling from {} with user id: {user}; Need to run as either the calling user id ({calling}), or with {INTERACT_ACROSS_USERS_FULL} permission",
            package.unwrap_or("null")
        )))
    }

    /// `enforceMediaPermissions`.
    fn enforce_media_permissions(&self, package: Option<&str>, uid: i32, user: i32) -> Result<(), Exception> {
        if self.held(STATUS_BAR_SERVICE, uid)? || uid == SYSTEM_UID || self.held(MEDIA_CONTENT_CONTROL, uid)? {
            return Ok(());
        }
        // `hasEnabledNotificationListener`: only for the caller's own user.
        let allowed = match package {
            Some(package) if uid / PER_USER_RANGE as i32 == user => {
                let path = self.host.data()?.join("system/notification_policy.xml");
                let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
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
    fn verify_sessions_request(&self, component: Option<&ComponentName>, user: i32, uid: i32) -> Result<i32, Exception> {
        let package = component.map(|c| c.package.as_str());
        if package.is_some() {
            self.enforce_package_name(package, uid)?;
        }
        let user = self.incoming_user(uid, user, package)?;
        self.enforce_media_permissions(package, uid, user)?;
        Ok(user)
    }

    /// `getFullUserRecordLocked`: the app host's users have no profiles, so
    /// each is its own full user.
    fn full_user(&self, user: i32) -> Result<Option<i32>, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.users.contains_key(&user).then_some(user))
    }

    /// Keeps `listener` under `key` in the map `pick` chooses, linked to its
    /// death unless `link` is off.
    fn keep<K: Copy + Eq + std::hash::Hash + Send + 'static>(
        &self,
        handle: u32,
        key: K,
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
        pick(&mut self.listeners.lock().unwrap()).insert(key, Kept { strong, death });
    }

    fn forget<K: Eq + std::hash::Hash>(&self, key: &K, pick: fn(&mut Listeners) -> &mut HashMap<K, Kept>) {
        if let Some(kept) = pick(&mut self.listeners.lock().unwrap()).remove(key)
            && let Some(death) = kept.death
        {
            self.host.process.clear_death(&kept.strong, death);
        }
    }

    /// `getMediaKeyEventSessionPackageName`: no media button session, so
    /// the user's last media button receiver's package, else empty.
    fn key_event_session_package(&self, uid: i32, package: Option<&str>) -> Result<String, Exception> {
        let user = uid / PER_USER_RANGE as i32;
        self.enforce_package_name(package, uid)?;
        self.enforce_media_permissions(package, uid, user)?;
        if self.full_user(user)?.is_none() {
            return Ok(String::new());
        }
        let info = self.host.inputs()?.settings.get(Table::Secure, MEDIA_BUTTON_RECEIVER);
        Ok(media_button_receiver_package(info.as_deref())?.unwrap_or_default())
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
            sm::GET_SESSIONS => {
                let a = sm::GetSessions::<ComponentName>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.verify_sessions_request(a.comp_name.as_ref(), a.user_id, uid)
                    .map(|_| sm::write_get_sessions_reply::<NoToken>(&mut p, Some(&[])))
            }
            sm::ADD_SESSIONS_LISTENER => {
                let a = sm::AddSessionsListener::<ComponentName>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let Some(Binder::Handle(handle)) = a.listener else {
                        eprintln!("app host: media_session: addSessionsListener: listener is null, ignoring");
                        return Ok(());
                    };
                    self.verify_sessions_request(a.comp_name.as_ref(), a.user_id, uid)?;
                    if self.listeners.lock().unwrap().sessions.contains_key(&handle) {
                        eprintln!("app host: media_session: ActiveSessionsListener is already added, ignoring");
                        return Ok(());
                    }
                    self.keep(handle, handle, true, sessions);
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
                (|| {
                    let user = uid / PER_USER_RANGE as i32;
                    self.enforce_package_name(a.package_name.as_deref(), uid)?;
                    self.enforce_media_permissions(a.package_name.as_deref(), uid, user)?;
                    sm::write_get_media_key_event_session_reply::<NoToken>(&mut p, None);
                    Ok(())
                })()
            }
            sm::GET_MEDIA_KEY_EVENT_SESSION_PACKAGE_NAME => {
                let a = sm::GetMediaKeyEventSessionPackageName::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.key_event_session_package(uid, a.package_name.as_deref())
                    .map(|package| sm::write_get_media_key_event_session_package_name_reply(&mut p, &Some(package)))
            }
            sm::ADD_ON_MEDIA_KEY_EVENT_SESSION_CHANGED_LISTENER => {
                let a = sm::AddOnMediaKeyEventSessionChangedListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
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
                    self.keep(handle, (user, handle), true, key_event_session);
                    Ok(())
                })()
                .map(|()| sm::write_add_on_media_key_event_session_changed_listener_reply(&mut p))
            }
            sm::REMOVE_ON_MEDIA_KEY_EVENT_SESSION_CHANGED_LISTENER => {
                let a = sm::RemoveOnMediaKeyEventSessionChangedListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
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
                let a = sm::AddSession2TokensListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let Some(Binder::Handle(handle)) = a.listener else {
                        eprintln!("app host: media_session: addSession2TokensListener: listener is null, ignoring");
                        return Ok(());
                    };
                    self.incoming_user(uid, a.user_id, None)?;
                    if self.listeners.lock().unwrap().session2_tokens.contains_key(&handle) {
                        eprintln!("app host: media_session: addSession2TokensListener: listener is already added, ignoring");
                        return Ok(());
                    }
                    self.keep(handle, handle, false, session2_tokens);
                    Ok(())
                })()
                .map(|()| sm::write_add_session2tokens_listener_reply(&mut p))
            }
            sm::REMOVE_SESSION2TOKENS_LISTENER => {
                let a = sm::RemoveSession2TokensListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(handle)) = a.listener {
                    self.forget(&handle, session2_tokens);
                }
                sm::write_remove_session2tokens_listener_reply(&mut p);
                Ok(())
            }
            sm::IS_GLOBAL_PRIORITY_ACTIVE => {
                sm::IsGlobalPriorityActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                sm::write_is_global_priority_active_reply(&mut p, false);
                Ok(())
            }
            code => {
                let name = sm::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("media_session", &format!("{name} (#1793)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MediaButtonReceiverHolder.unflattenFromString`: a component, a
    /// user and an optional type; anything else is no receiver, and a bad
    /// number throws.
    #[test]
    fn media_button_receivers_unflatten_as_the_holder_does() {
        let package = |s: &str| media_button_receiver_package(Some(s));
        assert_eq!(package("com.example/.Receiver,0").unwrap().as_deref(), Some("com.example"));
        assert_eq!(package("com.example/com.example.Service,0,3").unwrap().as_deref(), Some("com.example"));
        assert_eq!(package("com.example/.Receiver").unwrap(), None);
        assert_eq!(package("com.example,0").unwrap(), None);
        assert_eq!(package("a/b,0,1,2").unwrap(), None);
        assert_eq!(media_button_receiver_package(Some("")).unwrap(), None);
        assert_eq!(media_button_receiver_package(None).unwrap(), None);
        assert!(package("com.example/.Receiver,x").is_err());
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
}
