//! Native install-session state, ported from android-16.0.0_r1
//! PackageInstallerSession (AOSP, Apache-2.0). Installation and durable
//! recovery must be connected before publishing PackageInstaller (#986).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

pub mod codec;
pub mod endpoint;
pub mod service;

const INSTALL_APEX: i32 = 0x00020000;
const INSTALL_ENABLE_ROLLBACK: i32 = 0x00040000;
pub const INSTALL_REQUEST_UPDATE_OWNERSHIP: i32 = 1 << 25;

#[derive(Clone, Debug, PartialEq)]
pub struct Parameters {
    pub multi_package: bool,
    pub staged: bool,
    pub install_flags: i32,
    pub application_enabled_setting_persistent: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: i32,
    pub installer_uid: u32,
    pub user: u32,
    pub parameters: Parameters,
    pub parent: i32,
    pub children: BTreeSet<i32>,
    pub active_count: i32,
    pub prepared: bool,
    pub sealed: bool,
    pub destroyed: bool,
    pub client_progress: f32,
    pub reported_progress: f32,
}

impl Session {
    fn owner(&self, uid: u32) -> Result<(), Exception> {
        if uid == 0 || uid == self.installer_uid {
            Ok(())
        } else {
            Err(Exception::security("Session does not belong to caller"))
        }
    }

    fn mutable(&self) -> Result<(), Exception> {
        if !self.prepared {
            Err(state("Session is not prepared"))
        } else if self.destroyed || self.sealed {
            Err(Exception::security("Session is destroyed or sealed"))
        } else {
            Ok(())
        }
    }

    pub fn progress(&self) -> f32 {
        (self.client_progress * 0.8).clamp(0.0, 0.8)
    }
}

fn state(message: &str) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<i32, Session>,
    allocated: BTreeSet<i32>,
    records: BTreeMap<i32, Record>,
}

#[derive(Default)]
pub struct Sessions(Mutex<State>);

impl Sessions {
    /// The service supplies validated, normalized parameters after its
    /// permission, app-ops, user and policy checks. Allocated ids are never
    /// reused during this owner's lifetime, including after abandonment.
    pub fn create(
        &self,
        installer_uid: u32,
        user: u32,
        parameters: Parameters,
        install_permission: bool,
    ) -> Result<i32, Exception> {
        let mut state = self.0.lock().unwrap();
        let active = state
            .sessions
            .values()
            .filter(|s| s.installer_uid == installer_uid)
            .count();
        if active >= if install_permission { 1024 } else { 50 } {
            return Err(self::state("Too many active sessions"));
        }
        for _ in 0..33 {
            let id = unsafe { libc::arc4random_uniform(i32::MAX as u32 - 1) } as i32 + 1;
            if state.allocated.insert(id) {
                state.sessions.insert(
                    id,
                    Session {
                        id,
                        installer_uid,
                        user,
                        parameters,
                        parent: -1,
                        children: BTreeSet::new(),
                        active_count: 0,
                        prepared: false,
                        sealed: false,
                        destroyed: false,
                        client_progress: 0.0,
                        reported_progress: 0.0,
                    },
                );
                return Ok(id);
            }
        }
        Err(self::state("Failed to allocate session ID"))
    }

    /// Full normalized parameters and install source accompany the native lifecycle.
    pub fn create_record(
        &self,
        record: Record,
        install_permission: bool,
    ) -> Result<i32, Exception> {
        if record.params.has_capabilities() {
            return Err(Exception::new(aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,"native install file capability owner unavailable"));
        }
        let id = self.create(
            record.installer_uid,
            record.user,
            Parameters {
                multi_package: record.params.multi_package,
                staged: record.params.staged,
                install_flags: record.params.install_flags,
                application_enabled_setting_persistent: record
                    .params
                    .application_enabled_setting_persistent,
            },
            install_permission,
        )?;
        self.0.lock().unwrap().records.insert(id, record);
        Ok(id)
    }
    pub fn records(&self) -> Vec<(Session, Record)> {
        let state = self.0.lock().unwrap();
        state
            .records
            .iter()
            .filter_map(|(id, record)| {
                state
                    .sessions
                    .get(id)
                    .map(|session| (session.clone(), record.clone()))
            })
            .collect()
    }

    pub fn snapshot(&self, id: i32) -> Result<Session, Exception> {
        self.0
            .lock()
            .unwrap()
            .sessions
            .get(&id)
            .cloned()
            .ok_or_else(|| state("Unknown session"))
    }

    /// Preparation is performed by the storage owner first. A caller must
    /// never publish prepared=true before its stage directory exists.
    pub fn prepared(&self, id: i32) -> Result<(), Exception> {
        self.change(id, |s| {
            if s.destroyed {
                return Err(state("Destroyed session"));
            }
            s.prepared = true;
            Ok(())
        })
    }

    pub fn open(&self, id: i32, uid: u32) -> Result<Option<Event>, Exception> {
        self.change(id, |s| {
            s.owner(uid)?;
            if !s.prepared || s.destroyed {
                return Err(state("Session is unavailable"));
            }
            s.active_count = s
                .active_count
                .checked_add(1)
                .ok_or_else(|| state("Session active count overflow"))?;
            Ok((s.active_count == 1).then_some(Event::Active {
                id,
                user: s.user,
                active: true,
            }))
        })
    }

    /// Caller access is checked by IPackageInstaller; sealed verifier access
    /// does not transfer session ownership to the verifier.
    pub fn open_authorized(&self, id: i32) -> Result<Option<Event>, Exception> {
        self.change(id, |session| {
            if !session.prepared || session.destroyed {
                return Err(state("Session is unavailable"));
            }
            session.active_count = session
                .active_count
                .checked_add(1)
                .ok_or_else(|| state("Session active count overflow"))?;
            Ok((session.active_count == 1).then_some(Event::Active {
                id,
                user: session.user,
                active: true,
            }))
        })
    }

    pub fn close(&self, id: i32, uid: u32) -> Result<Option<Event>, Exception> {
        self.change(id, |s| {
            s.owner(uid)?;
            s.active_count = s.active_count.wrapping_sub(1);
            Ok((s.active_count == 0).then_some(Event::Active {
                id,
                user: s.user,
                active: false,
            }))
        })
    }

    pub fn progress(
        &self,
        id: i32,
        uid: u32,
        progress: f32,
        add: bool,
    ) -> Result<Option<Event>, Exception> {
        self.change(id, |s| {
            s.owner(uid)?;
            let force = s.client_progress == 0.0;
            s.client_progress = if add {
                s.client_progress + progress
            } else {
                progress
            };
            let progress = s.progress();
            if force || progress - s.reported_progress >= 0.01 {
                s.reported_progress = progress;
                Ok(Some(Event::Progress {
                    id,
                    user: s.user,
                    progress,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn add_child(&self, id: i32, child: i32, uid: u32) -> Result<(), Exception> {
        let mut state = self.0.lock().unwrap();
        let parent = state
            .sessions
            .get(&id)
            .ok_or_else(|| self::state("Unknown parent"))?;
        let target = state
            .sessions
            .get(&child)
            .ok_or_else(|| self::state("Unknown child"))?;
        if !parent.parameters.multi_package || target.parameters.multi_package {
            return Err(self::state("Invalid multi-package session graph"));
        }
        if parent.parameters.staged != target.parameters.staged
            || (parent.parameters.install_flags ^ target.parameters.install_flags)
                & INSTALL_ENABLE_ROLLBACK
                != 0
        {
            return Err(self::state("Inconsistent staged or rollback settings"));
        }
        let apex = target.parameters.install_flags & INSTALL_APEX != 0;
        if !parent.parameters.staged
            && parent
                .children
                .iter()
                .any(|id| (state.sessions[id].parameters.install_flags & INSTALL_APEX != 0) != apex)
        {
            return Err(self::state("Non-staged APK and APEX cannot be mixed"));
        }
        if target.destroyed || (target.parent != -1 && target.parent != id) {
            return Err(self::state("Child session is unavailable"));
        }
        parent.owner(uid)?;
        parent.mutable()?;
        if parent.children.contains(&child) {
            return Ok(());
        }
        state.sessions.get_mut(&id).unwrap().children.insert(child);
        state.sessions.get_mut(&child).unwrap().parent = id;
        Ok(())
    }

    pub fn remove_child(&self, id: i32, child: i32, uid: u32) -> Result<(), Exception> {
        let mut state = self.0.lock().unwrap();
        let parent = state
            .sessions
            .get_mut(&id)
            .ok_or_else(|| self::state("Unknown parent"))?;
        parent.owner(uid)?;
        parent.mutable()?;
        if parent.children.remove(&child) {
            state.sessions.get_mut(&child).unwrap().parent = -1;
        }
        Ok(())
    }

    /// Marking destruction returns every affected id to the storage and
    /// callback owners; cleanup and onSessionFinished are not implied.
    pub fn abandon(&self, id: i32, uid: u32) -> Result<Vec<i32>, Exception> {
        let mut state = self.0.lock().unwrap();
        let session = state
            .sessions
            .get(&id)
            .ok_or_else(|| self::state("Unknown session"))?;
        if session.parent != -1 {
            return Err(self::state("Child session cannot be abandoned"));
        }
        if uid != 1000 {
            session.owner(uid)?;
        }
        if session.destroyed {
            return Ok(Vec::new());
        }
        let ids: Vec<_> = std::iter::once(id)
            .chain(session.children.iter().copied())
            .collect();
        for id in &ids {
            state.sessions.get_mut(id).unwrap().destroyed = true;
        }
        Ok(ids)
    }

    fn change<T>(
        &self,
        id: i32,
        f: impl FnOnce(&mut Session) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let mut state = self.0.lock().unwrap();
        f(state
            .sessions
            .get_mut(&id)
            .ok_or_else(|| self::state("Unknown session"))?)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Active { id: i32, user: u32, active: bool },
    Progress { id: i32, user: u32, progress: f32 },
}

pub struct SessionNode {
    pub sessions: Arc<Sessions>,
    pub id: i32,
    /// Called after releasing the session lock; the owner must enqueue the
    /// event for registered Binder callbacks.
    pub notify: Arc<dyn Fn(Event) + Send + Sync>,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn params(multi_package: bool, flags: i32) -> Parameters {
        Parameters {
            multi_package,
            staged: false,
            install_flags: flags,
            application_enabled_setting_persistent: false,
        }
    }
    #[test]
    fn graph_is_atomic_and_abandon_reaches_children() {
        let sessions = Sessions::default();
        let parent = sessions.create(10200, 0, params(true, 0), false).unwrap();
        let child = sessions.create(10200, 0, params(false, 0), false).unwrap();
        let apex = sessions
            .create(10200, 0, params(false, INSTALL_APEX), false)
            .unwrap();
        sessions.prepared(parent).unwrap();
        assert!(sessions.add_child(parent, child, 1000).is_err());
        assert_eq!(sessions.snapshot(child).unwrap().parent, -1);
        sessions.add_child(parent, child, 10200).unwrap();
        sessions.add_child(parent, child, 10200).unwrap();
        assert!(sessions.add_child(parent, apex, 10200).is_err());
        assert_eq!(sessions.snapshot(apex).unwrap().parent, -1);
        assert!(sessions.abandon(child, 10200).is_err());
        assert_eq!(sessions.abandon(parent, 1000).unwrap(), vec![parent, child]);
        assert!(sessions.snapshot(child).unwrap().destroyed);
    }
    #[test]
    fn opening_requires_preparation_and_progress_matches_original_weight() {
        let sessions = Sessions::default();
        let id = sessions.create(10200, 10, params(false, 0), false).unwrap();
        assert!(sessions.open(id, 10200).is_err());
        sessions.prepared(id).unwrap();
        sessions.open(id, 10200).unwrap();
        assert!(sessions.progress(id, 1000, 1.0, false).is_err());
        sessions.progress(id, 10200, 0.5, false).unwrap();
        assert_eq!(sessions.snapshot(id).unwrap().progress(), 0.4);
        sessions.progress(id, 10200, 1.0, true).unwrap();
        assert_eq!(sessions.snapshot(id).unwrap().progress(), 0.8);
        sessions.close(id, 10200).unwrap();
        assert_eq!(sessions.snapshot(id).unwrap().active_count, 0);
    }
    #[test]
    fn binder_rejects_trailing_data_before_mutation_and_emits_callbacks() {
        use aim_binder_host::local::{Call, Service};
        use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader};
        use aim_service_aidl::android_content_pm_ipackageinstallersession as aidl;
        let sessions = Arc::new(Sessions::default());
        let id = sessions.create(10200, 0, params(false, 0), false).unwrap();
        sessions.prepared(id).unwrap();
        sessions.open(id, 10200).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let saved = events.clone();
        let node = SessionNode {
            sessions: sessions.clone(),
            id,
            notify: Arc::new(move |event| saved.lock().unwrap().push(event)),
        };
        let mut parcel = Parcel::new();
        aidl::SetClientProgress { progress: 0.5 }.write(&mut parcel);
        parcel.write_i32(1);
        let mut call = Call {
            code: aidl::SET_CLIENT_PROGRESS,
            flags: 0,
            sender_pid: 1,
            sender_euid: 10200,
            data: Reader::new(parcel.data(), parcel.objects()),
        };
        assert_eq!(node.transact(&mut call).unwrap_err(), BAD_VALUE);
        assert_eq!(sessions.snapshot(id).unwrap().client_progress, 0.0);
        let mut parcel = Parcel::new();
        aidl::SetClientProgress { progress: 0.5 }.write(&mut parcel);
        call.data = Reader::new(parcel.data(), parcel.objects());
        let reply = node.transact(&mut call).unwrap();
        assert!(
            aidl::read_set_client_progress_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .is_ok()
        );
        assert_eq!(
            *events.lock().unwrap(),
            vec![Event::Progress {
                id,
                user: 0,
                progress: 0.4
            }]
        );
    }
}

/// Creation source is separate from the mutable lifecycle. Inputs have passed
/// native caller/user/app-ops/storage normalization before they reach this owner.
#[derive(Clone, Debug)]
pub struct Record {
    pub params: codec::SessionParams,
    pub installer_uid: u32,
    pub user: u32,
    pub installer_package: Option<String>,
    pub installer_attribution_tag: Option<String>,
    pub created_millis: i64,
}
impl Record {
    pub fn info(
        &self,
        session: &Session,
        include_icon: bool,
        calling_uid: u32,
    ) -> codec::SessionInfo {
        let scrub = calling_uid >= 10000 && calling_uid != session.installer_uid;
        codec::SessionInfo {
            session_id: session.id,
            user_id: session.user as i32,
            installer_package_name: self.installer_package.clone(),
            installer_attribution_tag: self.installer_attribution_tag.clone(),
            progress: session.progress(),
            sealed: session.sealed,
            active: session.active_count > 0,
            mode: self.params.mode,
            install_reason: self.params.install_reason,
            install_scenario: self.params.install_scenario,
            size_bytes: self.params.size_bytes,
            app_package_name: self.params.app_package_name.clone(),
            app_icon: include_icon.then(|| self.params.app_icon.clone()).flatten(),
            app_label: self.params.app_label.clone(),
            install_location: self.params.install_location,
            originating_uri: (!scrub)
                .then(|| self.params.originating_uri.clone())
                .flatten(),
            originating_uid: self.params.originating_uid,
            referrer_uri: (!scrub).then(|| self.params.referrer_uri.clone()).flatten(),
            granted_runtime_permissions: (self.params.install_flags & 0x100 == 0).then(|| {
                self.params
                    .permission_states
                    .iter()
                    .filter(|(_, state)| *state == Some(1))
                    .map(|(name, _)| name.clone())
                    .collect()
            }),
            whitelisted_restricted_permissions: self
                .params
                .whitelisted_restricted_permissions
                .clone(),
            auto_revoke_permissions_mode: self.params.auto_revoke_permissions_mode,
            install_flags: self.params.install_flags,
            multi_package: self.params.multi_package,
            staged: self.params.staged,
            parent_session_id: session.parent,
            child_session_ids: Some(session.children.iter().copied().collect()),
            rollback_data_policy: self.params.rollback_data_policy,
            rollback_lifetime_millis: self.params.rollback_lifetime_millis,
            rollback_impact_level: self.params.rollback_impact_level,
            created_millis: self.created_millis,
            require_user_action: self.params.require_user_action,
            installer_uid: self.installer_uid as i32,
            package_source: self.params.package_source,
            application_enabled_setting_persistent: self
                .params
                .application_enabled_setting_persistent,
            auto_installing_dependencies_enabled: self.params.auto_install_dependencies_enabled,
            session_error_code: 0,
            session_error_message: Some(String::new()),
            ..Default::default()
        }
    }
}
