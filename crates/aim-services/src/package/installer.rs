//! Native install-session state, ported from android-16.0.0_r1
//! PackageInstallerSession (AOSP, Apache-2.0). Installation and durable
//! recovery must be connected before publishing PackageInstaller (#986).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

pub mod callbacks;
pub mod archiver;
pub mod archive_queries;
mod archive_timer;
pub mod checksums;
pub mod commit;
pub mod codec;
pub mod internal_installs;
pub mod endpoint;
pub mod environment;
pub mod post_install;
pub mod environment_image;
pub mod environment_producers;
pub mod existing;
pub mod file_bridge;
pub mod hardlink;
pub mod native;
pub mod move_pipeline;
pub mod pipeline;
pub mod permission_prepare;
pub mod permission_capture;
pub mod app_data;
pub mod preapproval;
pub mod constraints;
pub mod policy;
pub mod removal;
pub mod service;
pub mod silent;
pub mod storage;
pub mod staged_owner;
pub mod streaming_owner;

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
pub struct InstallationFile {
    pub location: i32,
    pub name: Option<String>,
    pub length: i64,
    pub metadata: Option<Vec<u8>>,
    pub signature: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: i32,
    pub installer_uid: u32,
    pub original_installer_uid: u32,
    pub committed: bool,
    pub committed_millis: i64,
    pub resolved_package:Option<String>,
    pub validated_target_sdk:Option<i32>,
    pub checksums: checksums::Pending,
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
    pub has_app_metadata: bool,
    pub pre_verified_domains: Option<Vec<String>>,
    pub installation_files: Vec<InstallationFile>,
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
        } else if self.destroyed || self.sealed || self.committed {
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
    historical: Vec<(Session, Record)>,
    bound: BTreeMap<i32, Weak<BoundSession>>,
}

/// One original Session object may outlive removal from PIS's active map.
/// Only live Binder nodes hold this owner; the registry keeps a weak index.
pub(crate) struct BoundSession {
    id: i32,
    registry: Weak<Sessions>,
    terminal: Mutex<Option<Session>>,
}
impl Drop for BoundSession {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            let mut state = registry.0.lock().unwrap();
            if state.bound.get(&self.id).is_some_and(|owner| std::ptr::eq(owner.as_ptr(), self)) {
                state.bound.remove(&self.id);
            }
        }
    }
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
                        original_installer_uid: installer_uid,
                        committed: false,
                        committed_millis: 0,
                        resolved_package:None,validated_target_sdk:None,
                        checksums: Default::default(),
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
                        has_app_metadata: false,
                        pre_verified_domains: None,
                        installation_files: Vec::new(),
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
            return Err(Exception::new(
                aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                "native install file capability owner unavailable",
            ));
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
    pub fn update_icon(&self,id:i32,uid:u32,icon:Option<codec::Object>)->Result<(),Exception>{
        let mut state=self.0.lock().unwrap();state.sessions.get(&id).ok_or_else(||Exception::security(format!("Caller has no access to session {id}")))?.owner(uid)?;
        state.records.get_mut(&id).ok_or_else(||self::state("Session parameter owner unavailable"))?.params.app_icon=icon;Ok(())
    }
    pub fn update_label(
        &self,
        id: i32,
        uid: u32,
        label: Option<String>,
    ) -> Result<Option<Event>, Exception> {
        let mut state = self.0.lock().unwrap();
        let session = state
            .sessions
            .get(&id)
            .ok_or_else(|| Exception::security(format!("Caller has no access to session {id}")))?;
        if uid != 0 && uid != session.installer_uid {
            return Err(Exception::security(format!(
                "Caller has no access to session {id}"
            )));
        }
        let user = session.user;
        let label = label.ok_or_else(|| {
            Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null appLabel")
        })?;
        let record = state
            .records
            .get_mut(&id)
            .ok_or_else(|| self::state("session parameter owner unavailable"))?;
        if record.params.app_label.as_deref() == Some(&label) {
            return Ok(None);
        }
        record.params.app_label = Some(label);
        Ok(Some(Event::Badging { id, user }))
    }
    pub fn restore(&self, records: Vec<(Session, Record)>) -> Result<(), Exception> {
        let mut state = self.0.lock().unwrap();
        if !state.sessions.is_empty() || !state.allocated.is_empty() {
            return Err(self::state("installer recovery requires fresh owner"));
        }
        let mut next = State::default();
        for (session, record) in records {
            if session.id <= 0
                || !next.allocated.insert(session.id)
                || session.installer_uid != record.installer_uid
                || session.user != record.user
            {
                return Err(self::state("invalid recovered session identity"));
            }
            next.records.insert(session.id, record);
            next.sessions.insert(session.id, session);
        }
        *state = next;
        Ok(())
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

    pub(crate) fn bind_session(self: &Arc<Self>, id: i32) -> Result<Arc<BoundSession>, Exception> {
        let mut state = self.0.lock().unwrap();
        if !state.sessions.contains_key(&id) { return Err(self::state("Unknown session")); }
        if let Some(owner) = state.bound.get(&id).and_then(Weak::upgrade) { return Ok(owner); }
        let owner = Arc::new(BoundSession {id,registry:Arc::downgrade(self),terminal:Mutex::new(None)});
        state.bound.insert(id, Arc::downgrade(&owner));
        Ok(owner)
    }
    fn check_bound(&self, id: i32, owner: &BoundSession) -> Result<(), Exception> {
        if owner.id != id || !std::ptr::eq(owner.registry.as_ptr(), self) {
            return Err(Exception::security("Foreign bound session owner"));
        }
        Ok(())
    }
    pub(crate) fn bound_snapshot(&self,id:i32,owner:&BoundSession)->Result<Session,Exception>{
        self.check_bound(id,owner)?;
        let state=self.0.lock().unwrap();
        if let Some(session)=state.sessions.get(&id){return Ok(session.clone());}
        owner.terminal.lock().unwrap().clone().ok_or_else(||self::state("Unknown bound session"))
    }
    pub(crate) fn close_bound(&self,id:i32,uid:u32,owner:&BoundSession)->Result<Option<Event>,Exception>{
        self.check_bound(id,owner)?;
        let mut state=self.0.lock().unwrap();
        let mut terminal=owner.terminal.lock().unwrap();
        let session=state.sessions.get_mut(&id).or(terminal.as_mut()).ok_or_else(||self::state("Unknown bound session"))?;
        session.owner(uid)?;
        session.active_count=session.active_count.wrapping_sub(1);
        Ok((session.active_count==0).then_some(Event::Active{id,user:session.user,active:false}))
    }

    /// Preparation is performed by the storage owner first. A caller must
    /// never publish prepared=true before its stage directory exists.
    pub fn prepared(&self, id: i32) -> Result<(), Exception> {
        self.change(id, |s| {
            s.prepared = true;
            Ok(())
        })
    }

    pub fn open(&self, id: i32, uid: u32) -> Result<Option<Event>, Exception> {
        self.change(id, |s| {
            s.owner(uid)?;
            if !s.prepared {
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
            if !session.prepared {
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
    pub fn record_historical(&self,id:i32)->Result<(),Exception> {
        let mut state=self.0.lock().unwrap();
        let root=state.sessions.get(&id).cloned().ok_or_else(||self::state("Unknown historical session"))?;
        let ids:Vec<_>=std::iter::once(id).chain(root.children.iter().copied()).collect();
        for id in ids {
            if state.historical.iter().any(|(session,_)|session.id==id){continue;}
            let session=state.sessions.get(&id).cloned().ok_or_else(||self::state("Historical child unavailable"))?;
            let record=state.records.get(&id).cloned().ok_or_else(||self::state("Historical record unavailable"))?;
            if state.historical.len()>500 {state.historical.drain(..400);}
            state.historical.push((session,record));
        }
        Ok(())
    }
    /// PIS.onSessionFinished removes ordinary root/child sessions from the
    /// active registry after notification. Allocated IDs and history survive.
    pub(crate) fn finish_nonstaged(&self, id: i32) -> Result<Vec<i32>, Exception> {
        let mut state = self.0.lock().unwrap();
        let root = state.sessions.get(&id).cloned().ok_or_else(|| self::state("Unknown finished session"))?;
        if root.parent != -1 { return Err(self::state("Finished child requires its root")); }
        if root.parameters.staged { return Ok(Vec::new()); }
        let ids = std::iter::once(id).chain(root.children.iter().copied()).collect::<Vec<_>>();
        let records = ids.iter().map(|id| {
            let session = state.sessions.get(id).cloned().ok_or_else(|| self::state("Finished child unavailable"))?;
            let record = state.records.get(id).cloned().ok_or_else(|| self::state("Finished parameter owner unavailable"))?;
            Ok((session,record))
        }).collect::<Result<Vec<_>,Exception>>()?;
        let bound = records.iter().filter_map(|(session,_)|state.bound.get(&session.id).and_then(Weak::upgrade)).collect::<Vec<_>>();
        for (session,record) in records {
            if !state.historical.iter().any(|(old,_)|old.id==session.id) {
                if state.historical.len()>500 {state.historical.drain(..400);}
                state.historical.push((session.clone(),record));
            }
            if let Some(owner)=bound.iter().find(|owner|owner.id==session.id) {
                let mut terminal=session.clone();terminal.destroyed=true;
                *owner.terminal.lock().unwrap()=Some(terminal);
            }
            state.sessions.remove(&session.id);
            state.records.remove(&session.id);
        }
        // A temporary upgrade may be the last strong owner if its Binder node
        // dies concurrently; release outside the registry lock.
        drop(state);
        drop(bound);
        Ok(ids)
    }
    pub fn historical_records(&self)->Vec<(Session,Record)> {self.0.lock().unwrap().historical.clone()}
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
        // Resolve all actual history records before changing the graph. A
        // missing child record must not partially destroy otherwise live nodes.
        let histories = ids.iter().map(|id| {
            let mut session = state.sessions.get(id).cloned().ok_or_else(|| self::state("Historical child unavailable"))?;
            let record = state.records.get(id).cloned().ok_or_else(|| self::state("Historical record unavailable"))?;
            session.destroyed = true;
            Ok((session, record))
        }).collect::<Result<Vec<_>, Exception>>()?;
        for id in &ids { state.sessions.get_mut(id).unwrap().destroyed = true; }
        for (session, record) in histories {
            if state.historical.iter().any(|(existing, _)| existing.id == session.id) { continue; }
            if state.historical.len() > 500 { state.historical.drain(..400); }
            state.historical.push((session, record));
        }
        Ok(ids)
    }

    pub(crate) fn transfer(
        &self,
        id: i32,
        uid: u32,
        destination: String,
        new_uid: u32,
        open: &dyn Fn(i32) -> bool,
    ) -> Result<(), Exception> {
        let mut state = self.0.lock().unwrap();
        let session = state
            .sessions
            .get_mut(&id)
            .ok_or_else(|| self::state("Unknown session"))?;
        session.owner(uid)?;
        session.mutable()?;
        if open(id) {
            return Err(Exception::security("Files still open"));
        }
        session.sealed = true;
        session.installer_uid = new_uid;
        let record = state
            .records
            .get_mut(&id)
            .ok_or_else(|| self::state("Session parameter owner unavailable"))?;
        record.installer_uid = new_uid;
        record.installer_package = Some(destination.clone());
        record.installer_package_uid = new_uid as i32;
        record.initiating_package = Some(destination);
        record.originating_package = None;
        record.installer_attribution_tag = None;
        Ok(())
    }
    pub(crate) fn seal(
        &self,
        id: i32,
        uid: u32,
        open: &dyn Fn(i32) -> bool,
    ) -> Result<Vec<i32>, (Vec<i32>, Exception)> {
        let mut state = self.0.lock().unwrap();
        let session = state
            .sessions
            .get(&id)
            .ok_or_else(|| (Vec::new(), self::state("Unknown session")))?;
        if session.parent != -1 {
            return Err((
                Vec::new(),
                self::state("seal can't be called on a child session"),
            ));
        }
        session.owner(uid).map_err(|error| (Vec::new(), error))?;
        let ids: Vec<_> = std::iter::once(id)
            .chain(session.children.iter().copied())
            .collect();
        let mut sealed = Vec::new();
        for id in ids {
            if open(id) {
                state.sessions.get_mut(&id).unwrap().destroyed = true;
                return Err((
                    vec![id],
                    self::state("Package is not valid: Files still open"),
                ));
            }
            let session = state.sessions.get_mut(&id).unwrap();
            if !session.prepared || session.destroyed {
                session.destroyed = true;
                return Err((vec![id], self::state("Package is not valid")));
            }
            session.sealed = true;
            sealed.push(id);
        }
        Ok(sealed)
    }
    pub(crate) fn with_record<T>(
        &self,
        id: i32,
        action: impl FnOnce(&Session, &Record) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let state = self.0.lock().unwrap();
        let session = state
            .sessions
            .get(&id)
            .ok_or_else(|| self::state("Unknown session"))?;
        let record = state
            .records
            .get(&id)
            .ok_or_else(|| self::state("Session parameter owner unavailable"))?;
        action(session, record)
    }
    pub(crate) fn with_record_mut<T>(
        &self,
        id: i32,
        action: impl FnOnce(&mut Session, &Record) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let mut state = self.0.lock().unwrap();
        let State {
            sessions, records, ..
        } = &mut *state;
        let session = sessions
            .get_mut(&id)
            .ok_or_else(|| self::state("Unknown session"))?;
        let record = records
            .get(&id)
            .ok_or_else(|| self::state("Session parameter owner unavailable"))?;
        action(session, record)
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
    Badging { id: i32, user: u32 },
    Created { id: i32, user: u32 },
    Finished { id: i32, user: u32, success: bool },
    Active { id: i32, user: u32, active: bool },
    Progress { id: i32, user: u32, progress: f32 },
}

pub trait SessionOperations: Send + Sync {
    fn commit_session(&self,id:i32,uid:u32,receiver:Option<preapproval::IntentSender>,transferred:bool)->Result<(),Exception>;
    fn request_preapproval(&self,id:i32,uid:u32,body:codec::Object)->Result<(),Exception>;
    fn stage_hard_link(&self, id: i32, uid: u32, target: Option<String>) -> Result<(), Exception>;
    fn set_checksums(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        checksums: Option<Vec<Option<checksums::Checksum>>>,
        signature: Option<Vec<u8>>,
    ) -> Result<(), Exception>;
    fn request_checksums(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        optional: i32,
        required: i32,
        trusted: checksums::TrustedInstallers,
        listener: Option<aim_binder_host::parcel::Binder>,
    ) -> Result<(), Exception>;
    fn fetch_package_names(&self, id: i32, uid: u32) -> Result<Vec<Option<String>>, Exception>;
    fn add_file(&self, id: i32, uid: u32, file: InstallationFile) -> Result<(), Exception>;
    fn remove_file(
        &self,
        id: i32,
        uid: u32,
        location: i32,
        name: Option<String>,
    ) -> Result<(), Exception>;
    fn pre_verified_domains(&self, id: i32, uid: u32) -> Result<Option<Vec<String>>, Exception>;
    fn set_pre_verified_domains(
        &self,
        id: i32,
        uid: u32,
        domains: Option<crate::package::domain_verification::domain_set::DomainSet>,
    ) -> Result<(), Exception>;
    fn metadata_read(
        &self,
        id: i32,
        uid: u32,
    ) -> Result<Option<aim_binder_driver::File>, Exception>;
    fn metadata_write(&self, id: i32, uid: u32) -> Result<aim_binder_driver::File, Exception>;
    fn metadata_remove(&self, id: i32) -> Result<(), Exception>;
    fn open_write(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        offset: i64,
        length: i64,
    ) -> Result<aim_binder_driver::File, Exception>;
    fn write_file(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        offset: i64,
        length: i64,
        fd: Option<u32>,
    ) -> Result<(), Exception>;
    fn names(&self, id: i32, uid: u32) -> Result<Vec<Option<String>>, Exception>;
    fn open_read(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
    ) -> Result<aim_binder_driver::File, Exception>;
    fn remove_split(&self, id: i32, uid: u32, name: Option<String>) -> Result<(), Exception>;
    fn abandon_session(&self, id: i32, uid: u32) -> Result<(), Exception>;
    fn data_loader(&self, id: i32, uid: u32) -> Result<Option<codec::Object>, Exception>;
    fn transfer_session(&self, id: i32, uid: u32, name: Option<String>) -> Result<(), Exception>;
    fn seal_session(&self, id: i32, uid: u32) -> Result<(), Exception>;
    fn graph_changed(&self) -> Result<(), Exception>;
}
pub struct SessionNode {
    pub(crate) bound: Arc<BoundSession>,
    pub operations: Option<Arc<dyn SessionOperations>>,
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
    fn failed_retained_session_can_reopen_for_abandon_without_becoming_mutable() {
        let sessions=Sessions::default();
        let id=sessions.create_record(Record {
            params:codec::SessionParams {mode:1,..Default::default()},
            installer_uid:2000,user:0,installer_package:Some("com.android.shell".into()),
            installer_attribution_tag:None,created_millis:77,initiating_package:Some("com.android.shell".into()),
            originating_package:None,installer_package_uid:2000,
        },false).unwrap();
        sessions.prepared(id).unwrap();
        sessions.open(id,2000).unwrap();sessions.close(id,2000).unwrap();
        // The real failed-install owner destroys/records the session before
        // its shell client reopens it to perform the abandonment cleanup.
        assert_eq!(sessions.abandon(id,1000).unwrap(),vec![id]);
        assert!(sessions.open(id,19000).is_err());
        assert!(matches!(sessions.open(id,2000).unwrap(),Some(Event::Active {active:true,..})));
        assert!(sessions.snapshot(id).unwrap().mutable().is_err());
        assert!(sessions.abandon(id,2000).unwrap().is_empty());
        assert!(matches!(sessions.close(id,2000).unwrap(),Some(Event::Active {active:false,..})));
        assert!(matches!(sessions.open_authorized(id).unwrap(),Some(Event::Active {active:true,..})));
        sessions.close(id,2000).unwrap();
        assert_eq!(sessions.historical_records().len(),1);
        assert!(sessions.snapshot(id).unwrap().destroyed);
    }
    #[test]
    fn finished_nonstaged_family_retires_active_records_and_keeps_ids_and_history() {
        let sessions=Sessions::default();
        let create=|multi_package,staged|sessions.create_record(Record {
            params:codec::SessionParams{mode:1,multi_package,staged,..Default::default()},
            installer_uid:2000,user:0,installer_package:Some("fixture.installer".into()),
            installer_attribution_tag:None,created_millis:77,initiating_package:Some("fixture.installer".into()),
            originating_package:None,installer_package_uid:2000,
        },false).unwrap();
        let parent=create(true,false);let child=create(false,false);let staged=create(false,true);
        sessions.prepared(parent).unwrap();sessions.add_child(parent,child,2000).unwrap();
        for id in [parent,child] {sessions.with_record_mut(id,|session,_|{session.prepared=true;session.sealed=true;session.committed=true;Ok(())}).unwrap();}
        sessions.record_historical(parent).unwrap();
        let history=sessions.historical_records();
        assert!(sessions.finish_nonstaged(child).is_err());
        assert_eq!(sessions.records().len(),3);
        assert_eq!(sessions.finish_nonstaged(parent).unwrap().into_iter().collect::<BTreeSet<_>>(),BTreeSet::from([parent,child]));
        assert_eq!(sessions.records().iter().map(|(session,_)|session.id).collect::<Vec<_>>(),vec![staged]);
        assert!(sessions.snapshot(parent).is_err());assert!(sessions.snapshot(child).is_err());
        let after=sessions.historical_records();assert_eq!(after.len(),history.len());
        assert!(after.iter().all(|(session,record)|session.committed&&session.installer_uid==record.installer_uid));
        assert!(sessions.finish_nonstaged(staged).unwrap().is_empty());assert!(sessions.snapshot(staged).is_ok());
        let state=sessions.0.lock().unwrap();assert!(state.allocated.contains(&parent)&&state.allocated.contains(&child));
        // The active session XML writer only receives records(), so neither
        // retired ordinary root nor its child can be restored as active.
        assert!(!state.records.contains_key(&parent)&&!state.records.contains_key(&child));
    }
    #[test]
    fn graph_is_atomic_and_abandon_reaches_children() {
        let sessions = Sessions::default();
        // Abandon records history from full normalized creation data. The
        // low-level lifecycle create() deliberately supplies no such data.
        let create = |multi_package, flags| sessions.create_record(Record {
            params: codec::SessionParams { mode: 1, multi_package, install_flags: flags, ..Default::default() },
            installer_uid: 10200, user: 0, installer_package: Some("fixture.installer".into()),
            installer_attribution_tag: None, created_millis: 77,
            initiating_package: Some("fixture.installer".into()), originating_package: None,
            installer_package_uid: 10200,
        }, false).unwrap();
        let parent = create(true, 0);
        let child = create(false, 0);
        let apex = create(false, INSTALL_APEX);
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
        let history = sessions.historical_records();
        assert_eq!(history.iter().map(|(session, _)| session.id).collect::<Vec<_>>(), vec![parent, child]);
        assert!(history.iter().all(|(session, record)| session.destroyed && session.installer_uid == 10200
            && record.installer_uid == 10200 && record.created_millis == 77
            && record.installer_package.as_deref() == Some("fixture.installer")));
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
    fn bound_binder_close_survives_terminal_retirement_with_real_owner_and_count() {
        use aim_binder_host::{local::{Call,Service},parcel::{Parcel,Reader,BAD_VALUE,EX_SECURITY}};
        use aim_service_aidl::android_content_pm_ipackageinstallersession as aidl;
        let sessions=Arc::new(Sessions::default());
        let id=sessions.create_record(Record{params:codec::SessionParams{mode:1,..Default::default()},
            installer_uid:2000,user:0,installer_package:Some("fixture.installer".into()),installer_attribution_tag:None,
            created_millis:1,initiating_package:None,originating_package:None,installer_package_uid:2000},false).unwrap();
        sessions.prepared(id).unwrap();sessions.open(id,2000).unwrap();sessions.open(id,2000).unwrap();
        sessions.with_record_mut(id,|session,_|{session.committed=true;session.sealed=true;Ok(())}).unwrap();
        let events=Arc::new(Mutex::new(Vec::new()));
        let make_node=||{let events=events.clone();Arc::new(SessionNode{bound:sessions.bind_session(id).unwrap(),
            operations:None,sessions:sessions.clone(),id,notify:Arc::new(move|event|events.lock().unwrap().push(event))})};
        let first=make_node();let second=make_node();assert!(Arc::ptr_eq(&first.bound,&second.bound));
        let lease=Arc::downgrade(&first.bound);
        sessions.finish_nonstaged(id).unwrap();assert!(sessions.snapshot(id).is_err());assert!(sessions.records().is_empty());
        assert!(sessions.bound_snapshot(id,&first.bound).unwrap().destroyed);
        let mut request=Parcel::new();aidl::Close{}.write(&mut request);
        let transact=|node:&SessionNode,uid,parcel:&Parcel|node.transact(&mut Call{code:aidl::CLOSE,flags:0,sender_pid:1,sender_euid:uid,data:Reader::new(parcel.data(),parcel.objects())});
        let denied=transact(&first,10199,&request).unwrap();assert_eq!(denied.reader().read_exception().unwrap().unwrap_err().code,EX_SECURITY);
        assert_eq!(sessions.bound_snapshot(id,&first.bound).unwrap().active_count,2);assert!(events.lock().unwrap().is_empty());
        let mut malformed=Parcel::new();aidl::Close{}.write(&mut malformed);malformed.write_i32(1);
        assert_eq!(transact(&first,2000,&malformed).unwrap_err(),BAD_VALUE);
        let reply=transact(&first,2000,&request).unwrap();reply.reader().read_exception().unwrap().unwrap();
        assert_eq!(sessions.bound_snapshot(id,&first.bound).unwrap().active_count,1);assert!(events.lock().unwrap().is_empty());
        let reply=transact(&second,2000,&request).unwrap();reply.reader().read_exception().unwrap().unwrap();
        assert_eq!(sessions.bound_snapshot(id,&second.bound).unwrap().active_count,0);
        assert!(matches!(events.lock().unwrap().as_slice(),[Event::Active{id:returned,active:false,..}]if *returned==id));
        let mut read=Parcel::new();aidl::IsMultiPackage{}.write(&mut read);
        let reply=first.transact(&mut Call{code:aidl::IS_MULTI_PACKAGE,flags:0,sender_pid:1,sender_euid:2000,data:Reader::new(read.data(),read.objects())}).unwrap();
        assert!(!aidl::read_is_multi_package_reply(&mut reply.reader()).unwrap().unwrap());
        drop(first);assert!(lease.upgrade().is_some());drop(second);assert!(lease.upgrade().is_none());
        assert!(!sessions.0.lock().unwrap().bound.contains_key(&id));
        assert!(sessions.records().is_empty());assert_eq!(sessions.historical_records().len(),1);
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
            bound: sessions.bind_session(id).unwrap(),
            operations: None,
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
    pub initiating_package: Option<String>,
    pub originating_package: Option<String>,
    pub installer_package_uid: i32,
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
            app_package_name: session.resolved_package.clone().or_else(||self.params.app_package_name.clone()),
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
            committed: session.committed,
            session_error_code: 0,
            session_error_message: Some(String::new()),
            ..Default::default()
        }
    }
}

impl Event {
    pub fn identity(&self) -> (i32, u32) {
        match *self {
            Self::Created { id, user }
            | Self::Badging { id, user }
            | Self::Active { id, user, .. }
            | Self::Progress { id, user, .. }
            | Self::Finished { id, user, .. } => (id, user),
        }
    }
}

pub mod install_events;
