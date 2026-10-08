//! Concrete native policy, persistence and stage owners for install sessions.
use super::{
    Event, Record, Session, SessionNode, Sessions,
    codec::SessionParams,
    endpoint::Owners,
    policy::{self, DevicePolicy},
    storage,
};
use crate::package::{model::State, query::Query, resolve::Resolver, system_config::SystemConfig};
use aim_binder_host::{
    local::{LocalProcess, Service},
    parcel::{Binder, EX_ILLEGAL_STATE, Exception},
};
use std::sync::{Arc, Mutex};
#[path = "native_verification.rs"]
mod native_verification;
pub type StagedExecutor = Arc<
    dyn Fn(
            i32,
            Vec<(Session, Record, String)>,
        ) -> Result<crate::package::staging::ReadySession, Exception>
        + Send
        + Sync,
>;
pub type StreamPreparation =
    Arc<dyn Fn(&Session, &Record, &str) -> Result<bool, Exception> + Send + Sync>;
pub type QuerySource = Arc<dyn Fn() -> Result<Arc<State>, Exception> + Send + Sync>;
pub type Publisher = Arc<dyn Fn(Arc<dyn Service>) -> Result<Binder, Exception> + Send + Sync>;
pub type PolicySource = Arc<dyn Fn(u32, i32) -> Result<DevicePolicy, Exception> + Send + Sync>;
pub type WriteModeSource = Arc<dyn Fn() -> Result<bool, Exception> + Send + Sync>;
pub type AllocationOwner =
    Arc<dyn Fn(&std::fs::File, i64, i32) -> Result<(), Exception> + Send + Sync>;
#[derive(Clone)]
struct InstallConfig {
    apks: Arc<crate::package::write::Apks>,
    owners: Arc<dyn super::pipeline::Owners>,
}
#[derive(Clone)]
struct WriterConfig {
    mode: WriteModeSource,
    allocation: AllocationOwner,
}
pub struct LitePolicy {
    pub environment: crate::package::parse::lite::Environment,
    pub art_managed_extensions: Vec<String>,
}
pub type LitePolicySource = Arc<dyn Fn() -> Result<LitePolicy, Exception> + Send + Sync>;
pub type DomainPolicySource =
    Arc<dyn Fn() -> Result<(i64, i64, Option<String>), Exception> + Send + Sync>;
pub type Callback = Arc<dyn Fn(Event) + Send + Sync>;
pub struct NativeOwners {
    pub sessions: Arc<Sessions>,
    source: QuerySource,
    resolver: Resolver,
    policy_source: PolicySource,
    service_policy: Mutex<policy::ServicePolicy>,
    silent_policy: Mutex<super::silent::Policy>,
    callbacks: super::callbacks::Registry,
    config: SystemConfig,
    disk: Arc<Mutex<storage::Store>>,
    publisher: Publisher,
    callback: Callback,
    errors: Mutex<Vec<String>>,
    this: std::sync::Weak<Self>,
    process: std::sync::Weak<LocalProcess>,
    writer_config: Mutex<Option<WriterConfig>>,
    domain_policy: Mutex<Option<DomainPolicySource>>,
    lite_policy: Mutex<Option<LitePolicySource>>,
    install_config: Mutex<Option<InstallConfig>>,
    file_owners: Mutex<Option<Arc<super::hardlink::Files>>>,
    staging: Mutex<Option<Arc<crate::package::staging::Owner>>>,
    staged_executor: Mutex<Option<StagedExecutor>>,
    stream_preparation: Mutex<Option<StreamPreparation>>,
    production_staged: Mutex<Option<Arc<super::staged_owner::Owner>>>,
    production_stream: Mutex<Option<Arc<super::streaming_owner::Owner>>>,
    external: Mutex<Option<Arc<super::preapproval::BridgeOwner>>>,
    user_action: Mutex<Option<super::preapproval::UserAction>>,
    existing_owner: Mutex<Option<Arc<super::existing::Owner>>>,
    archiver: Mutex<Option<Arc<super::archiver::Owner>>>,
    removal: Mutex<Option<Arc<super::removal::Controller>>>,
    pending_icons: Mutex<Vec<(Option<super::codec::Object>, Option<Vec<u8>>)>>,
    approvals: Mutex<Option<Arc<super::preapproval::Owner>>>,
    approval_worker: Mutex<Option<super::preapproval::Worker>>,
    constraints: Mutex<Option<Arc<super::constraints::Owner>>>,
    constraint_worker: Mutex<Option<super::constraints::Worker>>,
    commit_policy: Mutex<Option<super::commit::PolicySource>>,
    confirmation_policy: Mutex<Option<super::commit::ConfirmationPolicySource>>,
    commit_queue: Mutex<Option<std::sync::mpsc::Sender<Option<i32>>>>,
    commit_worker: Mutex<Option<super::commit::Worker>>,
    receivers: Mutex<
        std::collections::BTreeMap<
            i32,
            (
                super::preapproval::IntentSender,
                Option<aim_binder_host::local::Strong>,
            ),
        >,
    >,
    writes: Arc<super::file_bridge::Registry>,
}
fn disk_error(error: storage::Error) -> Exception {
    Exception::new(
        EX_ILLEGAL_STATE,
        format!(
            "Native installer persistence: committed={}: {}",
            error.committed, error.message
        ),
    )
}
impl NativeOwners {
    pub fn system_config(&self) -> &SystemConfig {
        &self.config
    }
    pub(crate) fn commit_configured(&self) -> bool {
        self.commit_policy.lock().unwrap().is_some()
    }
    pub fn open(
        sessions: Arc<Sessions>,
        source: QuerySource,
        policy_source: PolicySource,
        config: SystemConfig,
        disk: storage::Store,
        publisher: Publisher,
        callback: Callback,
        process: Arc<LocalProcess>,
    ) -> Result<Arc<Self>, Exception> {
        let recovered = disk.recovered().map_err(disk_error)?;
        sessions.restore(recovered)?;
        let weak_owner = Arc::new(Mutex::new(std::sync::Weak::<Self>::new()));
        let weak = weak_owner.clone();
        let visibility: super::callbacks::VisibilitySource = Arc::new(move || {
            let owner = weak.lock().unwrap().upgrade().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "installer callback owner unavailable")
            })?;
            let state = (owner.source)()?;
            let resolution = owner.resolver.resolution(&state).map_err(|error| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("callback query resolution: {error:?}"),
                )
            })?;
            let sessions = owner.sessions.clone();
            Ok(Arc::new(move |uid, id| {
                let Some((session, record)) = sessions
                    .records()
                    .into_iter()
                    .find(|(session, _)| session.id == id)
                else {
                    return Ok(true);
                };
                if session.installer_uid == uid {
                    return Ok(true);
                }
                policy::can_query(
                    &Query {
                        state: &state,
                        filter: &resolution.apps_filter,
                        calling_uid: uid as i32,
                    },
                    record.params.app_package_name.as_deref(),
                )
            }) as super::callbacks::Visibility)
        });
        let weak_process = Arc::downgrade(&process);
        let callbacks = super::callbacks::Registry::new(process, visibility)?;
        let owner = Arc::new_cyclic(|this| Self {
            this: this.clone(),
            process: weak_process,
            writer_config: Mutex::new(None),
            domain_policy: Mutex::new(None),
            lite_policy: Mutex::new(None),
            install_config: Mutex::new(None),
            file_owners: Mutex::new(None),
            staging: Mutex::new(None),
            staged_executor: Mutex::new(None),
            stream_preparation: Mutex::new(None),
            production_staged: Mutex::new(None),
            production_stream: Mutex::new(None),
            external: Mutex::new(None),
            user_action: Mutex::new(None),
            existing_owner: Mutex::new(None),
            archiver: Mutex::new(None),
            removal: Mutex::new(None),
            pending_icons: Mutex::new(Vec::new()),
            approvals: Mutex::new(None),
            approval_worker: Mutex::new(None),
            constraints: Mutex::new(None),
            constraint_worker: Mutex::new(None),
            commit_policy: Mutex::new(None),
            confirmation_policy: Mutex::new(None),
            commit_queue: Mutex::new(None),
            commit_worker: Mutex::new(None),
            receivers: Mutex::new(Default::default()),
            writes: Arc::new(Default::default()),
            sessions,
            source,
            resolver: Resolver::default(),
            policy_source,
            service_policy: Mutex::new(Default::default()),
            silent_policy: Mutex::new(Default::default()),
            callbacks,
            config,
            disk: Arc::new(Mutex::new(disk)),
            publisher,
            callback,
            errors: Mutex::new(Vec::new()),
        });
        *weak_owner.lock().unwrap() = Arc::downgrade(&owner);
        Ok(owner)
    }

    pub fn free_stage_dirs(&self, volume: Option<&str>) -> Result<(), Exception> {
        if volume.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Adopted-volume staging owner unavailable",
            ));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "Installer clock precedes epoch"))?
            .as_millis() as i64;
        let stages = self
            .disk
            .lock()
            .unwrap()
            .staging_directories()
            .map_err(disk_error)?;
        let records = self.sessions.records();
        let mut keep = std::collections::BTreeSet::new();
        let mut abandon = std::collections::BTreeSet::new();
        for (session, record) in &records {
            if session.parameters.multi_package || record.params.volume_uuid.as_deref() != volume {
                continue;
            }
            let stage = self
                .disk
                .lock()
                .unwrap()
                .validation_path(session, record)
                .map_err(disk_error)?;
            if stages.contains(&stage) {
                if now.saturating_sub(record.created_millis) >= 8 * 60 * 60 * 1000 {
                    let root = if session.parent == -1 {
                        session.id
                    } else {
                        session.parent
                    };
                    if records
                        .iter()
                        .any(|(session, _)| session.id == root && !session.destroyed)
                    {
                        abandon.insert(root);
                    }
                } else {
                    keep.insert(stage);
                }
            }
        }
        for root in abandon {
            let ids = self.sessions.abandon(root, 1000)?;
            self.abandon_stage(&ids)?;
        }
        for stage in stages {
            if !keep.contains(&stage) {
                self.disk
                    .lock()
                    .unwrap()
                    .remove_staging_directory(&stage)
                    .map_err(disk_error)?;
            }
        }
        Ok(())
    }
    /// Internal archiver uses the same real session admission, persistence and
    /// stage owners as public installs; no parallel session registry is created.
    pub fn archiver_record(
        &self,
        params: SessionParams,
        installer: &str,
        user: i32,
    ) -> Result<Record, Exception> {
        let uid = self.query(1000, |query| {
            query
                .package_uid_internal(installer, 0, user, 1000)
                .map_err(policy::unknown)
        })?;
        if uid < 0 {
            return Err(super::archiver::name_error("Installer package unavailable"));
        }
        self.normalize(uid as u32, params, Some(installer.to_owned()), None, user)
            .map(|(record, _)| record)
    }
    pub fn archived_install_record(
        &self,
        params: SessionParams,
        installer: &str,
        user: i32,
        calling_uid: u32,
    ) -> Result<Record, Exception> {
        self.normalize(calling_uid, params, Some(installer.to_owned()), None, user)
            .map(|(record, _)| record)
    }
    pub fn archiver_save_sessions(&self) -> Result<(), Exception> {
        self.persist()
    }
    pub fn archiver_prepare_stage(
        &self,
        session: &Session,
        record: &Record,
    ) -> Result<String, Exception> {
        self.prepare_stage(session, record)?;
        self.disk
            .lock()
            .unwrap()
            .stage_path(session, record)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(disk_error)
    }
    pub fn archiver_installation_owner(
        &self,
    ) -> Result<Arc<dyn super::pipeline::Owners>, Exception> {
        self.install_config
            .lock()
            .unwrap()
            .as_ref()
            .map(|config| config.owners.clone())
            .ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "native archived installation owner unavailable",
                )
            })
    }
    pub fn archiver_device_policy(&self, uid: u32, user: i32) -> Result<DevicePolicy, Exception> {
        (self.policy_source)(uid, user)
    }

    pub fn configure_archiver(&self, owner: Arc<super::archiver::Owner>) -> Result<(), Exception> {
        let mut current = self.archiver.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Archiver already configured",
            ));
        }
        *self.removal.lock().unwrap() = Some(owner.removal.clone());
        *current = Some(owner);
        Ok(())
    }
    pub fn configure_removal(
        &self,
        owner: Arc<super::removal::Controller>,
    ) -> Result<(), Exception> {
        let mut current = self.removal.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Removal owner already configured",
            ));
        }
        *current = Some(owner);
        Ok(())
    }
    pub fn removal_owner(&self) -> Result<Arc<super::removal::Controller>, Exception> {
        self.removal
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Removal owner unavailable"))
    }
    pub fn set_existing_owner(&self, owner: Arc<super::existing::Owner>) -> Result<(), Exception> {
        let mut current = self.existing_owner.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Existing install owner already configured",
            ));
        }
        *current = Some(owner);
        Ok(())
    }
    pub fn existing_owner(&self) -> Result<Arc<super::existing::Owner>, Exception> {
        self.existing_owner
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Existing install owner unavailable"))
    }
    pub fn restore_icons(
        &self,
        decode: Arc<dyn Fn(&[u8]) -> Result<Option<super::codec::Object>, Exception> + Send + Sync>,
    ) -> Result<(), Exception> {
        let icons = self
            .disk
            .lock()
            .unwrap()
            .recovered_icons()
            .map_err(disk_error)?;
        for (id, bytes) in icons {
            if self.sessions.snapshot(id).is_ok() {
                self.sessions.update_icon(id, 0, decode(&bytes)?)?;
            }
        }
        Ok(())
    }
    pub fn confirmation_stage_path(
        &self,
        session: &Session,
        record: &Record,
    ) -> Result<String, Exception> {
        self.disk
            .lock()
            .unwrap()
            .validation_path(session, record)
            .map_err(disk_error)
    }

    pub fn configure_confirmation(
        &self,
        policy: super::commit::ConfirmationPolicySource,
    ) -> Result<(), Exception> {
        let mut current = self.confirmation_policy.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Confirmation policy already configured",
            ));
        }
        *current = Some(policy);
        Ok(())
    }
    pub fn confirmation_source(&self) -> super::preapproval::UserAction {
        let weak = self.this.clone();
        Arc::new(move |id, accepted| {
            weak.upgrade()
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Installer owner closed"))?
                .confirmation(id, accepted)
        })
    }
    fn confirmation(&self, id: i32, accepted: bool) -> Result<Option<String>, Exception> {
        if accepted {
            return Ok(None);
        }
        let (session, record) = self
            .sessions
            .records()
            .into_iter()
            .find(|(session, _)| session.id == id)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Unknown confirmation session"))?;
        if session.parameters.multi_package {
            return Ok(None);
        }
        let inputs = self
            .confirmation_policy
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "Installer managed/emergency/compat confirmation policy unavailable",
                )
            })?(&session, &record)?;
        self.query(session.installer_uid, |query| {
            let permission = |name| policy::permission(query, name);
            let package = session
                .resolved_package
                .as_deref()
                .or(record.params.app_package_name.as_deref());
            let target = package.and_then(|name| query.state.packages.get(name));
            let is_update = target.is_some() || session.parameters.install_flags & 0x20000 != 0;
            let self_update = target.is_some_and(|target| {
                session.installer_uid as i32 == session.user as i32 * 100000 + target.app_id
            });
            let owner = target.and_then(|target| target.install_source.update_owner.as_deref());
            let is_update_owner =
                owner.is_some_and(|owner| record.installer_package.as_deref() == Some(owner));
            let record_installer =
                target.and_then(|target| target.install_source.installer.as_deref());
            let installer_of_record = record_installer
                .is_some_and(|installer| record.installer_package.as_deref() == Some(installer));
            let force = session.parameters.install_flags & 0x400 != 0
                || record.params.require_user_action == 1;
            let prompt = || Ok(Some(inputs.installer_package.clone()));
            if matches!(session.installer_uid, 0 | 1000)
                || inputs.device_owner_or_affiliated
                || inputs.emergency_install
                || session.parameters.install_flags & (1 << 30) != 0
            {
                return if force { prompt() } else { Ok(None) };
            }
            if inputs.update_ownership_enabled
                && owner.is_some()
                && !is_update_owner
                && session.installer_uid != 2000
                && session.parameters.install_flags & (1 << 26) == 0
                && session.parameters.install_flags & 0x20000 == 0
            {
                return prompt();
            }
            let privileged = permission("android.permission.INSTALL_PACKAGES")?
                || is_update && permission("android.permission.INSTALL_PACKAGE_UPDATES")?
                || self_update && permission("android.permission.INSTALL_SELF_UPDATES")?
                || inputs.has_device_admin_receiver
                    && permission("android.permission.INSTALL_DPC_PACKAGES")?
                || inputs.dependency_installer_enabled
                    && inputs.is_sdk_or_static_library
                    && permission("android.permission.INSTALL_DEPENDENCY_SHARED_LIBRARIES")?;
            if privileged {
                return if force { prompt() } else { Ok(None) };
            }
            if inputs.install_disabled {
                return prompt();
            }
            if record.params.require_user_action == 2
                && permission("android.permission.UPDATE_PACKAGES_WITHOUT_USER_ACTION")?
                && (self_update
                    || if inputs.update_ownership_enabled && owner.is_some() {
                        is_update_owner
                    } else {
                        installer_of_record
                    })
                && inputs.silent_target_allowed
            {
                if let Some(package) = package {
                    if self.silent_update_allowed(
                        record.installer_package.as_deref(),
                        package,
                        inputs.uptime_millis,
                    ) {
                        self.track_silent_update(
                            record.installer_package.as_deref(),
                            package,
                            inputs.uptime_millis,
                        );
                        return Ok(None);
                    }
                }
            }
            prompt()
        })
    }
    pub fn configure_commit(&self, policy: super::commit::PolicySource) -> Result<(), Exception> {
        let mut current = self.commit_policy.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Installer commit policy already configured",
            ));
        }
        let weak = self.this.clone();
        let errors = self.this.clone();
        let run = Arc::new(move |id| {
            weak.upgrade()
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Installer owner closed"))?
                .run_install(id)
        });
        let failure = Arc::new(move |message| {
            if let Some(owner) = errors.upgrade() {
                owner.errors.lock().unwrap().push(message);
            }
        });
        let (queue, worker) = super::commit::Worker::start(run, failure);
        *self.commit_queue.lock().unwrap() = Some(queue);
        *self.commit_worker.lock().unwrap() = Some(worker);
        *current = Some(policy);
        Ok(())
    }
    pub fn resume_recovered_commits(&self) -> Result<(), Exception> {
        let queue = self.commit_queue.lock().unwrap().clone().ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "Installer recovery commit queue unavailable",
            )
        })?;
        if self.install_config.lock().unwrap().is_none() || self.external.lock().unwrap().is_none()
        {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Installer recovery owners not configured",
            ));
        }
        for (session, _) in self.sessions.records() {
            if session.parent == -1 && session.committed && !session.destroyed {
                queue.send(Some(session.id)).map_err(|_| {
                    Exception::new(EX_ILLEGAL_STATE, "Installer recovery worker stopped")
                })?;
            }
        }
        Ok(())
    }
    pub fn take_commit_worker(&self) -> Option<super::commit::Worker> {
        self.commit_worker.lock().unwrap().take()
    }
    fn deliver_pending_streaming(&self, id: i32, message: String) -> Result<(), Exception> {
        let receiver = self
            .receivers
            .lock()
            .unwrap()
            .get(&id)
            .map(|(receiver, _)| *receiver);
        if let Some(receiver) = receiver {
            self.external
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "stream status owner unavailable"))?
                .pending_streaming(receiver, id, &message)?;
        }
        Ok(())
    }
    fn deliver_install(
        &self,
        id: i32,
        legacy: i32,
        message: Option<String>,
    ) -> Result<(), Exception> {
        let sender = self
            .receivers
            .lock()
            .unwrap()
            .get(&id)
            .map(|(sender, _)| *sender);
        if let Some(receiver) = sender {
            let package = self.sessions.with_record(id, |session, record| {
                Ok(session
                    .resolved_package
                    .clone()
                    .or_else(|| record.params.app_package_name.clone()))
            })?;
            let external = self.external.lock().unwrap().clone().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "Installer status owner unavailable")
            })?;
            external.deliver(&super::preapproval::Status {
                receiver,
                session_id: id,
                package,
                legacy_status: legacy,
                message,
                preapproval: false,
                pending_installer: None,
            })?;
        }
        Ok(())
    }
    fn reject_install(&self, id: i32, message: &str) -> Result<(), Exception> {
        self.deliver_install(id, -115, Some(message.into()))?;
        let ids = self.sessions.abandon(id, 1000)?;
        self.abandon_stage(&ids)?;
        self.receivers.lock().unwrap().remove(&id);
        Ok(())
    }
    fn run_install(&self, id: i32) -> Result<(), Exception> {
        let result = self.install_staged_batch(id);
        match result {
            Err(super::pipeline::Error::Pending) => Ok(()),
            Ok(()) => {
                let root = self.sessions.snapshot(id)?;
                let ids: Vec<_> = std::iter::once(id)
                    .chain(root.children.iter().copied())
                    .collect();
                for member in ids {
                    let session = self.sessions.snapshot(member)?;
                    self.record_finished_install_history(member)?;
                    self.notify(Event::Finished {
                        id: member,
                        user: session.user,
                        success: true,
                    });
                }
                self.deliver_install(id, 1, None)?;
                self.receivers.lock().unwrap().remove(&id);
                let retired = self.sessions.finish_nonstaged(id)?;
                if !retired.is_empty() {
                    self.persist()?;
                }
                Ok(())
            }
            Err(super::pipeline::Error::Install(failure)) => {
                self.deliver_install(id, failure.legacy_status, Some(failure.message.clone()))?;
                if !failure.committed {
                    let ids = self.sessions.abandon(id, 1000)?;
                    self.abandon_stage(&ids)?;
                }
                self.receivers.lock().unwrap().remove(&id);
                Err(Exception::new(EX_ILLEGAL_STATE, failure.message))
            }
            Err(super::pipeline::Error::Owner { committed, mut error }) => {
                error.message = format!("Native installer session {id} owner failure (committed={committed}, exception {}): {}", error.code, error.message);
                self.deliver_install(id, -110, Some(error.message.clone()))?;
                if !committed {
                    let ids = self.sessions.abandon(id, 1000)?;
                    self.abandon_stage(&ids)?;
                }
                self.receivers.lock().unwrap().remove(&id);
                Err(error)
            }
        }
    }
    pub fn configure_production_preparation(
        self: &Arc<Self>,
        staged_bridge: aim_binder_host::local::Strong,
        stream_bridge: aim_binder_host::local::Strong,
        native: Arc<super::pipeline::Native>,
    ) -> Result<(), Exception> {
        let lite = self
            .lite_policy
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "APKLite policy unavailable"))?;
        let sessions = self.sessions.clone();
        let staged = super::staged_owner::Owner::new(
            staged_bridge,
            native,
            lite,
            self.disk.clone(),
            Arc::new(move || sessions.records()),
        )?;
        let weak = self.this.clone();
        let failures = self.this.clone();
        let pending = self.this.clone();
        let stream = super::streaming_owner::Owner::production(
            stream_bridge,
            self.process
                .upgrade()
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "installer process unavailable"))?,
            self.disk.clone(),
            self.publisher.clone(),
            Arc::new(move |id| {
                let owner = weak
                    .upgrade()
                    .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "installer owner detached"))?;
                let queue = owner.commit_queue.lock().unwrap().clone().ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, "commit handler unavailable")
                })?;
                queue
                    .send(Some(id))
                    .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "commit handler stopped"))
            }),
            Arc::new(move |id, status, message| {
                let owner = failures
                    .upgrade()
                    .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "installer owner detached"))?;
                owner.deliver_install(id, status, Some(message))
            }),
            Arc::new(move |id, message| {
                let owner = pending
                    .upgrade()
                    .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "installer owner detached"))?;
                owner.deliver_pending_streaming(id, message)
            }),
        );
        let mut executor = self.staged_executor.lock().unwrap();
        let mut preparation = self.stream_preparation.lock().unwrap();
        let mut staged_owner = self.production_staged.lock().unwrap();
        let mut stream_owner = self.production_stream.lock().unwrap();
        if executor.is_some()
            || preparation.is_some()
            || staged_owner.is_some()
            || stream_owner.is_some()
        {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "production preparation already configured",
            ));
        }
        *executor = Some(staged.executor());
        *preparation = Some(stream.preparation());
        *staged_owner = Some(staged);
        *stream_owner = Some(stream);
        Ok(())
    }
    pub fn configure_staged_executor(&self, executor: StagedExecutor) -> Result<(), Exception> {
        let mut current = self.staged_executor.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Staged executor already configured",
            ));
        }
        *current = Some(executor);
        Ok(())
    }
    pub fn configure_stream_preparation(&self, owner: StreamPreparation) -> Result<(), Exception> {
        let mut current = self.stream_preparation.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Data-loader preparation already configured",
            ));
        }
        *current = Some(owner);
        Ok(())
    }
    pub fn configure_staging(
        &self,
        staging: Arc<crate::package::staging::Owner>,
    ) -> Result<(), Exception> {
        let mut current = self.staging.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Installer staging owner already configured",
            ));
        }
        *current = Some(staging);
        Ok(())
    }
    pub fn configure_external(
        &self,
        external: Arc<super::preapproval::BridgeOwner>,
        user_action: super::preapproval::UserAction,
    ) -> Result<(), Exception> {
        let mut current = self.external.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Installer external owner already configured",
            ));
        }
        let weak = self.this.clone();
        let resume: super::preapproval::ResumeOwner = Arc::new(move |resume| {
            let owner = weak
                .upgrade()
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Installer owner closed"))?;
            match resume {
                super::preapproval::Resume::Install(id) => owner.run_install(id),
                super::preapproval::Resume::Rejected(id) => {
                    owner.reject_install(id, "User rejected permissions")
                }
                super::preapproval::Resume::Preapproval(_) => Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Preapproval resume belongs to its presentation queue",
                )),
            }
        });
        *self.user_action.lock().unwrap() = Some(user_action.clone());
        let (approvals, worker) =
            super::preapproval::Owner::start(external.clone(), resume, user_action);
        let (constraints, constraint_worker) =
            super::constraints::configure(external.clone(), &self.publisher)?;
        *self.constraints.lock().unwrap() = Some(constraints);
        *self.constraint_worker.lock().unwrap() = Some(constraint_worker);
        *self.approvals.lock().unwrap() = Some(approvals);
        *self.approval_worker.lock().unwrap() = Some(worker);
        *current = Some(external);
        Ok(())
    }
    pub fn take_constraint_worker(&self) -> Option<super::constraints::Worker> {
        self.constraint_worker.lock().unwrap().take()
    }
    fn external_owner(&self) -> Result<Arc<super::preapproval::BridgeOwner>, Exception> {
        self.external.lock().unwrap().clone().ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "Installer original SDK external owner unavailable",
            )
        })
    }
    pub fn take_approval_worker(&self) -> Option<super::preapproval::Worker> {
        self.approval_worker.lock().unwrap().take()
    }
    fn approval_owner(&self) -> Result<Arc<super::preapproval::Owner>, Exception> {
        self.approvals
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Installer approval owner unavailable"))
    }
    fn approval_session(&self, id: i32) -> Result<super::preapproval::Session, Exception> {
        let session = self.sessions.snapshot(id)?;
        Ok(super::preapproval::Session {
            id,
            installer_uid: session.installer_uid,
            prepared: session.prepared,
            sealed: session.sealed,
            destroyed: session.destroyed,
            multi_package: session.parameters.multi_package,
            committed: session.committed,
            parent: session.parent,
        })
    }
    pub fn configure_files(&self, files: Arc<super::hardlink::Files>) -> Result<(), Exception> {
        let mut current = self.file_owners.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Installer file owners already configured",
            ));
        }
        *current = Some(files);
        Ok(())
    }
    fn files(&self) -> Result<Arc<super::hardlink::Files>, Exception> {
        self.file_owners.lock().unwrap().clone().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                "Installer guest file/checksum owner unavailable",
            )
        })
    }
    pub fn configure_installation(
        &self,
        apks: Arc<crate::package::write::Apks>,
        owners: Arc<dyn super::pipeline::Owners>,
    ) -> Result<(), Exception> {
        let mut config = self.install_config.lock().unwrap();
        if config.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Native installation owners already configured",
            ));
        }
        *config = Some(InstallConfig { apks, owners });
        Ok(())
    }
    /// Called by the native commit handler after the session graph is sealed.
    /// The parser/verifier read actual staging files through the configured VFS.
    pub fn install_staged_batch(&self, id: i32) -> Result<(), super::pipeline::Error> {
        let failure = |message: &str| {
            super::pipeline::Error::Install(super::pipeline::Failure {
                legacy_status: -110,
                committed: false,
                message: message.into(),
            })
        };
        let config = self
            .install_config
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| failure("Native installation owners unavailable"))?;
        let records = self.sessions.records();
        let (root, _) = records
            .iter()
            .find(|(session, _)| session.id == id)
            .ok_or_else(|| failure("Unknown install root"))?;
        if root.parent != -1 || !root.prepared || !root.sealed || root.destroyed {
            return Err(failure("Install root is not an immutable prepared session"));
        }
        let ids: Vec<_> = if root.parameters.multi_package {
            root.children.iter().copied().collect()
        } else {
            vec![id]
        };
        if ids.is_empty() {
            return Err(failure("Multi-package install contains no children"));
        }
        let mut members = Vec::new();
        for id in ids {
            let (session, record) = records
                .iter()
                .find(|(session, _)| session.id == id)
                .ok_or_else(|| failure("Missing installation child"))?;
            let path = self
                .disk
                .lock()
                .unwrap()
                .validation_path(session, record)
                .map_err(|error| failure(&error.message))?;
            members.push((session.clone(), record.clone(), path));
        }
        if root.parameters.staged {
            let executor = self
                .staged_executor
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| {
                    failure("Actual staged verification/submission owner unavailable")
                })?;
            let ready = executor(id, members).map_err(|error| super::pipeline::Error::Owner {
                committed: false,
                error,
            })?;
            if ready.id != id {
                return Err(failure("Staged executor returned a foreign root"));
            }
            self.staging
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| failure("Native staging publication owner unavailable"))?
                .publish_ready(ready)
                .map_err(|error| super::pipeline::Error::Owner {
                    committed: false,
                    error,
                })?;
            self.deliver_install(id, 1, None)
                .map_err(|error| super::pipeline::Error::Owner {
                    committed: false,
                    error,
                })?;
            return Err(super::pipeline::Error::Pending);
        }
        for (session, record, path) in &members {
            if record.params.data_loader_params.is_some() {
                let owner = self
                    .stream_preparation
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| failure("Actual data-loader stream owner unavailable"))?;
                if !owner(session, record, path).map_err(|error| super::pipeline::Error::Owner {
                    committed: false,
                    error,
                })? {
                    return Err(super::pipeline::Error::Pending);
                }
            }
        }
        let lite = self
            .lite_policy
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| failure("Installer APKLite validation policy unavailable"))?;
        let lite = lite().map_err(|error| super::pipeline::Error::Owner {
            committed: false,
            error,
        })?;
        let files = self
            .files()
            .map_err(|error| super::pipeline::Error::Owner {
                committed: false,
                error,
            })?;
        for (session, record, _) in &members {
            let mut pending = session.checksums.clone();
            self.disk
                .lock()
                .unwrap()
                .normalize_apks(session, record, &lite, &mut pending, &files)
                .map_err(|error| failure(&error.message))?;
            if record.params.mode == 2 {
                let name = record
                    .params
                    .app_package_name
                    .as_ref()
                    .ok_or_else(|| failure("Missing existing package name"))?;
                let state = (self.source)().map_err(|error| super::pipeline::Error::Owner {
                    committed: false,
                    error,
                })?;
                let existing = state
                    .packages
                    .get(name)
                    .and_then(|package| package.pkg.as_ref())
                    .ok_or_else(|| failure("Missing existing base package"))?;
                self.disk
                    .lock()
                    .unwrap()
                    .inherit_existing(session, record, existing, &config.apks.files, &lite)
                    .map_err(|error| failure(&error.message))?;
            }
            self.sessions
                .with_record_mut(session.id, |current, _| {
                    current.checksums = pending;
                    Ok(())
                })
                .map_err(|error| super::pipeline::Error::Owner {
                    committed: false,
                    error,
                })?;
        }
        let code = super::pipeline::verify_batch(&config.apks, members, &lite)
            .map_err(super::pipeline::Error::Install)?;
        for member in &code {
            self.sessions
                .with_record_mut(member.session.id, |session, _| {
                    session.resolved_package = Some(member.package.package_name.clone());
                    session.validated_target_sdk = Some(member.package.target_sdk_version);
                    Ok(())
                })
                .map_err(|error| super::pipeline::Error::Owner {
                    committed: false,
                    error,
                })?;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| failure("Installer clock precedes epoch"))?
            .as_millis() as i64;
        let root = self
            .sessions
            .snapshot(id)
            .map_err(|error| super::pipeline::Error::Owner {
                committed: false,
                error,
            })?;
        for member in std::iter::once(id).chain(root.children.iter().copied()) {
            self.sessions
                .with_record_mut(member, |session, _| {
                    if !session.committed {
                        session.committed = true;
                        session.committed_millis = now;
                        session.active_count = session.active_count.wrapping_add(1);
                        session.client_progress = 1.0;
                    }
                    Ok(())
                })
                .map_err(|error| super::pipeline::Error::Owner {
                    committed: false,
                    error,
                })?;
        }
        self.persist()
            .map_err(|error| super::pipeline::Error::Owner {
                committed: false,
                error,
            })?;
        let action = self
            .user_action
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| failure("Installer user confirmation policy unavailable"))?;
        let approvals = self
            .approval_owner()
            .map_err(|error| super::pipeline::Error::Owner {
                committed: false,
                error,
            })?;
        for member in std::iter::once(id).chain(root.children.iter().copied()) {
            if let Some(installer) =
                action(member, approvals.manually_accepted(member)).map_err(|mut error| {
                    error.message = format!("Install user-action policy session {member} (exception {}): {}", error.code, error.message);
                    super::pipeline::Error::Owner {
                        committed: false,
                        error,
                    }
                })?
            {
                let receiver = self
                    .receivers
                    .lock()
                    .unwrap()
                    .get(&id)
                    .map(|(receiver, _)| *receiver)
                    .ok_or_else(|| failure("Install confirmation receiver unavailable"))?;
                self.external_owner()
                    .map_err(|error| super::pipeline::Error::Owner {
                        committed: false,
                        error,
                    })?
                    .deliver(&super::preapproval::Status {
                        receiver,
                        session_id: member,
                        package: self
                            .sessions
                            .with_record(member, |_, record| {
                                Ok(record.params.app_package_name.clone())
                            })
                            .map_err(|error| super::pipeline::Error::Owner {
                                committed: false,
                                error,
                            })?,
                        legacy_status: -1,
                        message: None,
                        preapproval: false,
                        pending_installer: Some(installer),
                    })
                    .map_err(|error| super::pipeline::Error::Owner {
                        committed: false,
                        error,
                    })?;
                return Err(super::pipeline::Error::Pending);
            }
        }
        super::pipeline::install(config.owners.as_ref(), code, &self.source)
    }
    pub fn configure_lite_policy(&self, source: LitePolicySource) -> Result<(), Exception> {
        let mut policy = self.lite_policy.lock().unwrap();
        if policy.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Installer APKLite policy already configured",
            ));
        }
        *policy = Some(source);
        Ok(())
    }
    pub fn configure_domain_policy(&self, source: DomainPolicySource) -> Result<(), Exception> {
        let mut policy = self.domain_policy.lock().unwrap();
        if policy.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Installer domain policy already configured",
            ));
        }
        *policy = Some(source);
        Ok(())
    }

    fn query<T>(
        &self,
        uid: u32,
        action: impl FnOnce(&Query<'_>) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let state = (self.source)()?;
        let resolution = self.resolver.resolution(&state).map_err(|error| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("native installer query resolution: {error:?}"),
            )
        })?;
        action(&Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid as i32,
        })
    }
    fn persist(&self) -> Result<(), Exception> {
        let records = self.sessions.records();
        self.disk
            .lock()
            .unwrap()
            .write(&records)
            .map_err(disk_error)
    }
    fn prepare_write(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        offset: i64,
        length: i64,
        reverse: bool,
    ) -> Result<(std::fs::File, Arc<super::file_bridge::Transfer>, bool), Exception> {
        let config = self.writer_config.lock().unwrap().clone().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                "Native installer writer mode/allocation owner unavailable",
            )
        })?;
        let (session, record, transfer) = self.sessions.with_record(id, |session, record| {
            if record.params.data_loader_params.is_some() {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Cannot write regular files in a data loader installation session.",
                ));
            }
            if uid != 0 && uid != session.installer_uid {
                return Err(Exception::security("Session does not belong to caller"));
            }
            if !session.prepared {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "assertCanWrite before prepared",
                ));
            }
            if session.destroyed || session.sealed {
                return Err(Exception::security(
                    "assertCanWrite not allowed after sealing or destruction",
                ));
            }
            if reverse && !matches!(uid, 0 | 1000 | 2000) {
                return Err(Exception::security(
                    "Reverse mode only supported from shell or system",
                ));
            }
            let transfer = self
                .writes
                .register(id)
                .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
            Ok((session.clone(), record.clone(), transfer))
        })?;
        let revocable = (config.mode)()?;
        let name = name
            .filter(|name| storage::valid_filename(name))
            .ok_or_else(|| Exception::illegal_argument("Invalid name"))?;
        let mut target = self
            .disk
            .lock()
            .unwrap()
            .write_target(&session, &record, &name, 0)
            .map_err(disk_error)?;
        if length > 0 {
            (config.allocation)(&target, length, record.params.install_flags)?;
        }
        if offset > 0 {
            use std::io::{Seek, SeekFrom};
            target
                .seek(SeekFrom::Start(offset as u64))
                .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
        }
        Ok((target, transfer, revocable))
    }
    pub fn configure_writer(
        &self,
        mode: WriteModeSource,
        allocation: AllocationOwner,
    ) -> Result<(), Exception> {
        let mut current = self.writer_config.lock().unwrap();
        if current.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "installer write owner already configured",
            ));
        }
        *current = Some(WriterConfig { mode, allocation });
        Ok(())
    }
    pub fn take_optional_io_worker_guard(
        &self,
    ) -> Result<Option<super::file_bridge::IoWorkerGuard>, Exception> {
        let configured = self.writer_config.lock().unwrap();
        if configured.is_none() {
            return Ok(None);
        }
        self.writes
            .take_guard()
            .map(Some)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "installer IO guard already taken"))
    }
    pub fn take_io_worker_guard(&self) -> Option<super::file_bridge::IoWorkerGuard> {
        self.writer_config.lock().unwrap().as_ref()?;
        self.writes.take_guard()
    }
    pub fn shutdown_io(&self) {
        self.writes.stop();
    }
    pub fn silent_update_allowed(
        &self,
        installer: Option<&str>,
        package: &str,
        uptime_ms: i64,
    ) -> bool {
        self.silent_policy
            .lock()
            .unwrap()
            .allowed(installer, package, uptime_ms)
    }
    pub fn track_silent_update(&self, installer: Option<&str>, package: &str, uptime_ms: i64) {
        self.silent_policy
            .lock()
            .unwrap()
            .track(installer, package, uptime_ms)
    }
    pub fn take_callback_worker(&self) -> Option<super::callbacks::CallbackWorker> {
        self.callbacks.take_worker()
    }
    pub fn close_external(&self) -> Result<(), Exception> {
        let mut failure = None;
        let stream = self.production_stream.lock().unwrap().take();
        if let Some(stream) = stream {
            if let Err(error) = stream.close() {
                self.errors.lock().unwrap().push(error.message.clone());
                failure = Some(error);
            }
        }
        let staged = self.production_staged.lock().unwrap().take();
        if let Some(staged) = staged {
            staged.close();
        }
        self.staged_executor.lock().unwrap().take();
        self.stream_preparation.lock().unwrap().take();
        if let Some(external) = self.external.lock().unwrap().clone() {
            if let Err(error) = external.close() {
                self.errors.lock().unwrap().push(error.message.clone());
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub fn shutdown_callbacks(&self) {
        if let Some(owner) = self.production_stream.lock().unwrap().as_ref() {
            owner.stop();
        }
        if let Some(owner) = self.production_staged.lock().unwrap().as_ref() {
            owner.stop();
        }
        self.shutdown_io();
        if let Some(queue) = self.commit_queue.lock().unwrap().take() {
            let _ = queue.send(None);
        }
        if let Some(approvals) = self.approvals.lock().unwrap().clone() {
            approvals.stop();
        }
        if let Some(constraints) = self.constraints.lock().unwrap().clone() {
            constraints.stop();
        }
        self.receivers.lock().unwrap().clear();
        self.callbacks.shutdown();
    }
    /// Asynchronous callback paths expose failed persistence to the service owner.
    pub fn take_errors(&self) -> Vec<String> {
        {
            let mut errors = std::mem::take(&mut *self.errors.lock().unwrap());
            errors.extend(self.callbacks.take_errors());
            errors.extend(self.writes.take_errors());
            if let Some(owner) = self.approvals.lock().unwrap().as_ref() {
                errors.extend(owner.errors());
            }
            if let Some(owner) = self.constraints.lock().unwrap().as_ref() {
                errors.extend(owner.errors());
            }
            errors
        }
    }
}
impl Owners for NativeOwners {
    fn staged_status(&self, id: i32) -> Result<Option<super::staged_owner::Status>, Exception> {
        let owner = self
            .production_staged
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "production staged owner unavailable")
            })?;
        Ok(owner.status(id))
    }

    fn uninstall(
        &self,
        request: super::removal::Request,
        receiver: Option<super::preapproval::IntentSender>,
    ) -> Result<(), Exception> {
        self.removal_owner()?.uninstall(request, receiver)
    }
    fn archive(
        &self,
        uid: u32,
        pid: i32,
        action: super::endpoint::ArchiveAction,
    ) -> Result<(), Exception> {
        let owner = self
            .archiver
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Archiver owner unavailable"))?;
        match action {
            super::endpoint::ArchiveAction::Archive {
                package,
                caller,
                flags,
                receiver,
                user,
            } => self.query(uid, |query| {
                owner.request_archive(query, &package, &caller, uid, pid, user, flags, receiver)
            }),
            super::endpoint::ArchiveAction::Unarchive {
                package,
                caller,
                receiver,
                user,
                show_confirmation,
            } => self.query(uid, |query| {
                owner.request_unarchive(
                    query,
                    &package,
                    &caller,
                    uid,
                    pid,
                    user,
                    receiver,
                    show_confirmation,
                )
            }),
            super::endpoint::ArchiveAction::Install {
                package,
                params,
                receiver,
                installer,
                user,
            } => self.query(uid, |query| {
                owner.install_archived(query, uid, pid, package, params, &installer, user, receiver)
            }),
            super::endpoint::ArchiveAction::Report {
                id,
                status,
                required,
                action,
                user,
            } => owner.report(uid, id, user, status, required, action),
        }
    }
    fn update_icon(&self, uid: u32, id: i32, body: super::codec::Object) -> Result<(), Exception> {
        let session = self.sessions.snapshot(id)?;
        session.owner(uid)?;
        let icon = self.external_owner()?.normalize_icon(&body)?;
        self.sessions.update_icon(id, uid, icon.parcel)?;
        self.disk
            .lock()
            .unwrap()
            .write_icon(id, icon.png.as_deref())
            .map_err(disk_error)?;
        self.persist()?;
        self.notify(Event::Badging {
            id,
            user: session.user,
        });
        Ok(())
    }
    fn install_constraints(
        &self,
        uid: u32,
        installer: Option<String>,
        names: Option<Vec<Option<String>>>,
        constraints: Option<super::constraints::Constraints>,
        receiver: super::endpoint::ConstraintReceiver,
        timeout: i64,
    ) -> Result<(), Exception> {
        let names = names
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_NULL_POINTER,
                    "null packageNames",
                )
            })?
            .into_iter()
            .map(|name| {
                name.ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_NULL_POINTER,
                        "null package name",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let constraints = constraints.ok_or_else(|| {
            Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null constraints")
        })?;
        self.query(uid, |query| {
            super::constraints::authorize(query, installer.as_deref(), &names)
        })?;
        let external = self.external_owner()?;
        let callback_names = names.clone();
        let weak = self.this.clone();
        let callback = Box::new(move |result: Result<bool, Exception>| {
            let result = result.and_then(|satisfied| match receiver {
                super::endpoint::ConstraintReceiver::Callback(callback) => {
                    external.constraint_callback(callback, satisfied)
                }
                super::endpoint::ConstraintReceiver::Intent(sender) => {
                    external.constraint_intent(sender, &callback_names, constraints.0, satisfied)
                }
            });
            if let Err(error) = result {
                if let Some(owner) = weak.upgrade() {
                    owner.errors.lock().unwrap().push(format!(
                        "Installer constraints completion: {}",
                        error.message
                    ));
                }
            }
        });
        self.constraints
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "Installer constraint scheduler unavailable",
                )
            })?
            .submit(names, constraints, timeout, callback)
    }
    fn install_existing(
        &self,
        uid: u32,
        pid: i32,
        request: super::existing::Request,
    ) -> Result<(), Exception> {
        self.existing_owner()?
            .install_existing(uid, pid, request)
            .map(|_| ())
    }
    fn permissions_result(&self, uid: u32, id: i32, accepted: bool) -> Result<(), Exception> {
        self.query(uid, |query| {
            if policy::permission(query, "android.permission.INSTALL_PACKAGES")? {
                Ok(())
            } else {
                Err(Exception::security("Requires INSTALL_PACKAGES"))
            }
        })?;
        if self.sessions.snapshot(id).is_err() {
            return Ok(());
        }
        self.approval_owner()?
            .permissions_result(&self.approval_session(id)?, accepted)
    }
    fn normalize(
        &self,
        uid: u32,
        mut params: SessionParams,
        installer: Option<String>,
        tag: Option<String>,
        user: i32,
    ) -> Result<(Record, bool), Exception> {
        let mut encoded_icon = None;
        if let Some(icon) = &params.app_icon {
            let mut reader = aim_binder_host::parcel::Reader::new(&icon.bytes, &icon.objects);
            reader.read_string16().map_err(|error| {
                Exception::new(EX_ILLEGAL_STATE, format!("Bitmap parcel: {error}"))
            })?;
            let start = reader.position();
            reader.skip(reader.remaining()).map_err(|error| {
                Exception::new(EX_ILLEGAL_STATE, format!("Bitmap parcel: {error}"))
            })?;
            let (bytes, objects) = reader.since(start);
            let mut parcel = aim_binder_host::parcel::Parcel::new();
            parcel.write_i32(1);
            parcel.write_raw(bytes, &objects);
            let normalized = self
                .external_owner()?
                .normalize_icon(&super::codec::Object {
                    bytes: parcel.data().to_vec(),
                    objects: parcel.objects().to_vec(),
                })?;
            params.app_icon = normalized.parcel;
            encoded_icon = normalized.png;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?
            .as_millis() as i64;
        let result = self.query(uid, |query| {
            policy::normalize(
                query,
                &(self.policy_source)(uid, user)?,
                &mut self.service_policy.lock().unwrap(),
                &self.config,
                params,
                installer,
                tag,
                user,
                now,
            )
        })?;
        self.disk
            .lock()
            .unwrap()
            .validate_storage(&result.0.params)
            .map_err(disk_error)?;
        if let Some(png) = encoded_icon {
            self.pending_icons
                .lock()
                .unwrap()
                .push((result.0.params.app_icon.clone(), Some(png)));
        }
        Ok(result)
    }
    fn session_created(&self, id: i32, user: u32) -> Result<(), Exception> {
        let icon = self
            .sessions
            .with_record(id, |_, record| Ok(record.params.app_icon.clone()))?;
        let png = {
            let mut pending = self.pending_icons.lock().unwrap();
            pending
                .iter()
                .position(|(candidate, _)| candidate == &icon)
                .map(|index| pending.remove(index).1)
        };
        if let Some(png) = png {
            self.disk
                .lock()
                .unwrap()
                .write_icon(id, png.as_deref())
                .map_err(disk_error)?;
        }
        self.persist()?;
        self.callbacks.notify(Event::Created { id, user })?;
        (self.callback)(Event::Created { id, user });
        Ok(())
    }
    fn prepare_stage(&self, session: &Session, record: &Record) -> Result<(), Exception> {
        self.disk
            .lock()
            .unwrap()
            .prepare_stage(session, record)
            .map_err(disk_error)
    }
    fn session_operations(&self) -> Option<Arc<dyn super::SessionOperations>> {
        self.this
            .upgrade()
            .map(|owner| owner as Arc<dyn super::SessionOperations>)
    }
    fn publish_session(&self, node: Arc<SessionNode>) -> Result<Binder, Exception> {
        (self.publisher)(node)
    }
    fn notify(&self, event: Event) {
        if let Err(error) = self.persist() {
            eprintln!(
                "native installer callback persistence failed: {}",
                error.message
            );
            self.errors.lock().unwrap().push(error.message);
        }
        if let Err(error) = self.callbacks.notify(event.clone()) {
            self.errors.lock().unwrap().push(error.message);
        }
        (self.callback)(event)
    }
    fn enforce_cross_user(&self, uid: u32, user: i32, operation: &str) -> Result<(), Exception> {
        self.query(uid, |query| {
            query
                .internal_enforce_cross_user(query.calling_uid, user, true, false, operation)
                .map_err(policy::unknown)?
        })
    }
    fn check_package(&self, uid: u32, package: Option<&str>) -> Result<(), Exception> {
        self.query(uid, |query| policy::check_package(query, package))
    }
    fn can_query(&self, uid: u32, package: Option<&str>) -> Result<bool, Exception> {
        self.query(uid, |query| policy::can_query(query, package))
    }
    fn can_read_paths(&self, uid: u32) -> Result<bool, Exception> {
        self.query(uid, |query| {
            policy::permission(query, "android.permission.READ_INSTALLED_SESSION_PATHS")
        })
    }
    fn resolved_path(&self, id: i32) -> Result<Option<String>, Exception> {
        let (session, record) = self
            .sessions
            .records()
            .into_iter()
            .find(|(session, _)| session.id == id)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Unknown session"))?;
        self.disk
            .lock()
            .unwrap()
            .resolved_path(&session, &record)
            .map_err(disk_error)
    }
    fn is_verifier(&self, uid: u32) -> Result<bool, Exception> {
        self.query(uid, |query| {
            policy::permission(query, "android.permission.PACKAGE_VERIFICATION_AGENT")
        })
    }
    fn update_label(&self, uid: u32, id: i32, label: Option<String>) -> Result<(), Exception> {
        if let Some(event) = self.sessions.update_label(id, uid, label)? {
            self.persist()?;
            self.callbacks.notify(event.clone())?;
            (self.callback)(event);
        }
        Ok(())
    }
    fn register_callback(
        &self,
        uid: u32,
        callback: Option<Binder>,
        user: i32,
    ) -> Result<(), Exception> {
        self.enforce_cross_user(uid, user, "registerCallback")?;
        self.callbacks.register(callback, uid, user)
    }
    fn unregister_callback(&self, callback: Option<Binder>) -> Result<(), Exception> {
        self.callbacks.unregister(callback)
    }
    fn set_service_policy(
        &self,
        uid: u32,
        setting: super::endpoint::ServiceSetting,
    ) -> Result<(), Exception> {
        if !matches!(uid, 0 | 1000 | 2000) {
            return Err(Exception::security("Operation not allowed for caller"));
        }
        match &setting {
            super::endpoint::ServiceSetting::Unlimited(installer) => {
                self.silent_policy
                    .lock()
                    .unwrap()
                    .set_unlimited(installer.clone());
                return Ok(());
            }
            super::endpoint::ServiceSetting::Throttle(seconds) => {
                self.silent_policy
                    .lock()
                    .unwrap()
                    .set_throttle_seconds(*seconds);
                return Ok(());
            }
            _ => {}
        }
        let mut policy = self.service_policy.lock().unwrap();
        match setting {
            super::endpoint::ServiceSetting::Unlimited(_)
            | super::endpoint::ServiceSetting::Throttle(_) => unreachable!(),
            super::endpoint::ServiceSetting::Staged(value) => {
                policy.bypass_next_staged_installer_check = value
            }
            super::endpoint::ServiceSetting::Apex(value) => {
                policy.bypass_next_allowed_apex_update_check = value
            }
            super::endpoint::ServiceSetting::Verification(uid) => {
                policy.disable_verification_for_uid = (uid != -1).then_some(uid)
            }
        }
        Ok(())
    }
    fn abandon_stage(&self, ids: &[i32]) -> Result<(), Exception> {
        self.writes.stop_sessions(ids);
        let records = self.sessions.records();
        let selected: Vec<_> = records
            .iter()
            .filter(|(session, _)| ids.contains(&session.id))
            .cloned()
            .collect();
        self.disk
            .lock()
            .unwrap()
            .abandon(&selected)
            .map_err(disk_error)?;
        self.persist()?;
        if let Some(staging) = self.staging.lock().unwrap().clone() {
            for id in ids {
                staging.remove(*id)?;
            }
        }
        if let Some(approvals) = self.approvals.lock().unwrap().clone() {
            for id in ids {
                approvals.remove(*id)?;
            }
        }
        for (session, _) in selected {
            self.callbacks.notify(Event::Finished {
                id: session.id,
                user: session.user,
                success: false,
            })?;
            (self.callback)(Event::Finished {
                id: session.id,
                user: session.user,
                success: false,
            })
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::model::{PackageState, PackageUserState, User};
    use aim_binder_host::{
        local::Call,
        parcel::{Parcel, Reader},
    };
    use aim_service_aidl::android_content_pm_ipackageinstaller as aidl;
    use std::collections::BTreeSet;
    struct Data(std::path::PathBuf);
    impl Data {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aim-native-installer-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            for name in ["system", "app", "app-staging"] {
                std::fs::create_dir_all(path.join(name)).unwrap();
            }
            Self(path)
        }
    }
    impl Drop for Data {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn state() -> Arc<State> {
        let mut state = State {
            packages: [(
                "fixture".into(),
                PackageState {
                    name: "fixture".into(),
                    app_id: 10100,
                    pkg: Some(Arc::new(crate::package::pkg::AndroidPackage {
                        package_name: "fixture".into(),
                        uid: 10100,
                        target_sdk_version: 35,
                        ..Default::default()
                    })),
                    users: [(
                        0,
                        PackageUserState {
                            granted_permissions: vec![
                                "android.permission.INTERACT_ACROSS_USERS".into(),
                            ],
                            ..Default::default()
                        },
                    )]
                    .into(),
                    ..Default::default()
                },
            )]
            .into(),
            users: [
                (
                    0,
                    User {
                        id: 0,
                        ..Default::default()
                    },
                ),
                (
                    10,
                    User {
                        id: 10,
                        ..Default::default()
                    },
                ),
            ]
            .into(),
            ..Default::default()
        };
        state.packages.insert(
            "android".into(),
            PackageState {
                name: "android".into(),
                app_id: 1000,
                pkg: Some(Arc::new(crate::package::pkg::AndroidPackage {
                    package_name: "android".into(),
                    uid: 1000,
                    target_sdk_version: 35,
                    requested_permissions: vec!["android.permission.QUERY_ALL_PACKAGES".into()],
                    ..Default::default()
                })),
                users: [(0, PackageUserState::default())].into(),
                ..Default::default()
            },
        );
        Arc::new(state)
    }
    fn device() -> DevicePolicy {
        DevicePolicy {
            debuggable: false,
            apex_supported: false,
            rollback_lifetime: true,
            users: [
                (
                    0,
                    policy::UserPolicy {
                        disallow_install_apps: false,
                        disallow_debugging_features: false,
                        organization_managed: false,
                    },
                ),
                (
                    10,
                    policy::UserPolicy {
                        disallow_install_apps: false,
                        disallow_debugging_features: false,
                        organization_managed: false,
                    },
                ),
            ]
            .into(),
            adopted_shell_uids: BTreeSet::new(),
            verifier_uid: None,
        }
    }
    fn disk(data: &Data) -> storage::Store {
        let inode = aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        };
        storage::Store::open(
            data.0.clone(),
            inode,
            aim_storage::guest_inode::GuestInode {
                mode: Some(0o775),
                ..inode
            },
            Arc::new(|path, guest| {
                use std::os::unix::ffi::OsStrExt;
                let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                let label = if guest.starts_with("/data/app/") {
                    b"u:object_r:apk_tmp_file:s0\0".as_slice()
                } else {
                    b"u:object_r:system_data_file:s0\0".as_slice()
                };
                if unsafe {
                    libc::setxattr(
                        path.as_ptr(),
                        c"dev.aim.xattr.security.selinux".as_ptr(),
                        label.as_ptr().cast(),
                        label.len(),
                        0,
                        libc::XATTR_NOFOLLOW,
                    )
                } != 0
                {
                    return Err(storage::Error {
                        committed: false,
                        message: std::io::Error::last_os_error().to_string(),
                    });
                }
                Ok(())
            }),
        )
        .unwrap()
    }
    fn make_owner(
        data: &Data,
        sessions: Arc<Sessions>,
        nodes: Arc<Mutex<Vec<Arc<dyn Service>>>>,
        events: Arc<Mutex<Vec<Event>>>,
    ) -> Arc<NativeOwners> {
        make_owner_with_policy(data, sessions, nodes, events, Arc::new(|_, _| Ok(device())))
    }
    fn make_owner_with_policy(
        data: &Data,
        sessions: Arc<Sessions>,
        nodes: Arc<Mutex<Vec<Arc<dyn Service>>>>,
        events: Arc<Mutex<Vec<Event>>>,
        policy_source: PolicySource,
    ) -> Arc<NativeOwners> {
        make_owner_with_source(
            data,
            sessions,
            nodes,
            events,
            policy_source,
            Arc::new(|| Ok(state())),
        )
    }
    fn make_owner_with_source(
        data: &Data,
        sessions: Arc<Sessions>,
        nodes: Arc<Mutex<Vec<Arc<dyn Service>>>>,
        events: Arc<Mutex<Vec<Event>>>,
        policy_source: PolicySource,
        query_source: QuerySource,
    ) -> Arc<NativeOwners> {
        NativeOwners::open(
            sessions,
            query_source,
            policy_source,
            SystemConfig::default(),
            disk(data),
            Arc::new(move |node| {
                let mut nodes = nodes.lock().unwrap();
                nodes.push(node);
                Ok(Binder::Local(nodes.len() as u64))
            }),
            Arc::new(move |event| events.lock().unwrap().push(event)),
            LocalProcess::open(
                &aim_binder_driver::Driver::new(),
                aim_binder_driver::Device::Binder,
                aim_binder_driver::Credentials {
                    pid: 77701,
                    euid: 1000,
                    security_context: None,
                },
            ),
        )
        .unwrap()
    }
    fn request() -> SessionParams {
        SessionParams {
            mode: 1,
            size_bytes: -1,
            originating_uid: -1,
            required_installed_version_code: -1,
            unarchive_id: -1,
            auto_install_dependencies_enabled: true,
            install_flags: 0x00400000,
            app_package_name: Some("fixture".into()),
            ..Default::default()
        }
    }
    fn call(endpoint: &dyn Service, code: u32, uid: u32, parcel: &Parcel) -> Parcel {
        endpoint
            .transact(&mut Call {
                code,
                flags: 0,
                sender_pid: 77,
                sender_euid: uid,
                data: Reader::new(parcel.data(), parcel.objects()),
            })
            .unwrap()
    }
    #[test]
    fn streaming_inventory_keeps_order_duplicate_identity_and_xml_bytes() {
        use super::super::SessionOperations;
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let owner = make_owner(
            &data,
            sessions.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        );
        let mut params = request();
        params.data_loader_params = Some(
            super::super::codec::DataLoader {
                kind: 1,
                package: Some("fixture".into()),
                class: Some("Loader".into()),
                arguments: Some("args".into()),
            }
            .object(),
        );
        let (record, permission) = owner
            .normalize(0, params, Some("fixture".into()), None, 0)
            .unwrap();
        let id = sessions.create_record(record, permission).unwrap();
        let (session, record) = sessions.records().pop().unwrap();
        owner.prepare_stage(&session, &record).unwrap();
        sessions.prepared(id).unwrap();
        let file = super::super::InstallationFile {
            location: 0,
            name: Some("base.apk".into()),
            length: -1,
            metadata: Some(vec![0, 255, 128]),
            signature: Some(vec![]),
        };
        assert!(owner.add_file(id, 10100, file.clone()).is_err());
        owner.add_file(id, 0, file.clone()).unwrap();
        assert!(owner.add_file(id, 0, file.clone()).is_err());
        let mut bad = file.clone();
        bad.location = 1;
        assert!(owner.add_file(id, 0, bad).is_err());
        owner.remove_file(id, 0, 1, Some("feature".into())).unwrap();
        assert!(owner.remove_file(id, 0, 1, Some("feature".into())).is_err());
        assert_eq!(
            owner.names(id, 0).unwrap(),
            [Some("base.apk".into()), Some("feature.removed".into())]
        );
        use aim_service_aidl::android_content_pm_ipackageinstallersession as session_api;
        let node = super::super::SessionNode {
            bound: sessions.bind_session(id).unwrap(),
            operations: Some(owner.clone()),
            sessions: sessions.clone(),
            id,
            notify: Arc::new(|_| {}),
        };
        let mut request = Parcel::new();
        session_api::AddFile {
            location: 0,
            name: Some("split.apk".into()),
            length_bytes: 41,
            metadata: Some(vec![]),
            signature: None,
        }
        .write(&mut request);
        request.write_i32(99);
        assert_eq!(
            node.transact(&mut Call {
                code: session_api::ADD_FILE,
                flags: 0,
                sender_pid: 77,
                sender_euid: 0,
                data: Reader::new(request.data(), request.objects())
            })
            .unwrap_err(),
            aim_binder_host::parcel::BAD_VALUE
        );
        assert_eq!(sessions.snapshot(id).unwrap().installation_files.len(), 2);
        owner.persist().unwrap();
        let recovered = owner.disk.lock().unwrap().recovered().unwrap();
        assert_eq!(
            recovered[0].0.installation_files,
            sessions.snapshot(id).unwrap().installation_files
        );
        assert_eq!(
            super::super::codec::DataLoader::from_object(
                recovered[0].1.params.data_loader_params.as_ref().unwrap()
            )
            .unwrap()
            .arguments
            .as_deref(),
            Some("args")
        );
        sessions.seal(id, 0, &|_| false).unwrap();
        assert!(owner.add_file(id, 0, file).is_err());
        owner.shutdown_callbacks();
    }
    #[test]
    fn preverified_domains_use_installer_identity_limits_and_recover_original_xml() {
        use super::super::SessionOperations;
        use crate::package::domain_verification::domain_set::DomainSet;
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let owner = make_owner(
            &data,
            sessions.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        );
        owner
            .configure_domain_policy(Arc::new(|| Ok((2, 3, None))))
            .unwrap();
        let (mut record, permission) = owner
            .normalize(0, request(), Some("fixture".into()), None, 0)
            .unwrap();
        record.installer_uid = 0;
        let id = sessions.create_record(record, permission).unwrap();
        assert!(owner.pre_verified_domains(id, 0).is_err());
        let (session, record) = sessions.records().pop().unwrap();
        owner.prepare_stage(&session, &record).unwrap();
        sessions.prepared(id).unwrap();
        assert!(
            owner
                .set_pre_verified_domains(
                    id,
                    10100,
                    Some(DomainSet::Hosts(vec![Some("ab".into())]))
                )
                .is_err()
        );
        assert!(
            owner
                .set_pre_verified_domains(id, 0, Some(DomainSet::Hosts(vec![Some("😀ab".into())])))
                .is_err()
        );
        owner
            .set_pre_verified_domains(
                id,
                0,
                Some(DomainSet::Hosts(vec![Some("BB".into()), Some("Aa".into())])),
            )
            .unwrap();
        assert_eq!(
            owner.pre_verified_domains(id, 0).unwrap().unwrap(),
            ["BB", "Aa"]
        );
        let node = super::super::SessionNode {
            bound: sessions.bind_session(id).unwrap(),
            operations: Some(owner.clone()),
            sessions: sessions.clone(),
            id,
            notify: Arc::new(|_| {}),
        };
        use aim_service_aidl::android_content_pm_ipackageinstallersession as session_api;
        let mut request = Parcel::new();
        session_api::GetPreVerifiedDomains {}.write(&mut request);
        let reply = call(&node, session_api::GET_PRE_VERIFIED_DOMAINS, 0, &request);
        let mut reader = Reader::new(reply.data(), reply.objects());
        reader.read_exception().unwrap().unwrap();
        assert_eq!(reader.read_i32().unwrap(), 1);
        assert!(!reader.read_bool().unwrap());
        assert_eq!(reader.read_i32().unwrap(), 2);
        assert_eq!(reader.read_string16().unwrap().as_deref(), Some("BB"));
        assert_eq!(reader.read_string16().unwrap().as_deref(), Some("Aa"));
        let mut request = Parcel::new();
        request.write_interface_token(session_api::DESCRIPTOR);
        request.write_i32(1);
        request.write_bool(false);
        request.write_i32(1);
        request.write_string16(Some("xy"));
        request.write_i32(99);
        assert_eq!(
            node.transact(&mut Call {
                code: session_api::SET_PRE_VERIFIED_DOMAINS,
                flags: 0,
                sender_pid: 77,
                sender_euid: 0,
                data: Reader::new(request.data(), request.objects())
            })
            .unwrap_err(),
            aim_binder_host::parcel::BAD_VALUE
        );
        assert_eq!(
            owner.pre_verified_domains(id, 0).unwrap().unwrap(),
            ["BB", "Aa"]
        );
        owner.persist().unwrap();
        let recovered = owner.disk.lock().unwrap().recovered().unwrap();
        assert_eq!(
            recovered[0].0.pre_verified_domains.as_ref().unwrap(),
            &["BB", "Aa"]
        );
        sessions.seal(id, 0, &|_| false).unwrap();
        assert!(
            owner
                .set_pre_verified_domains(id, 0, Some(DomainSet::Hosts(vec![])))
                .is_err()
        );
        assert!(owner.pre_verified_domains(id, 0).is_ok());
        owner.shutdown_callbacks();
    }
    #[test]
    fn native_binder_create_prepare_reopen_abandon_has_real_files_and_identity() {
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let owner = make_owner(&data, sessions.clone(), nodes.clone(), events.clone());
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut parcel = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: Some("tag".into()),
            user_id: 0,
        }
        .write(&mut parcel);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        let id = aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        assert!(data.0.join("system/install_sessions.xml").is_file());
        assert!(!data.0.join(format!("app/vmdl{id}.tmp")).exists());
        assert_eq!(
            events.lock().unwrap().as_slice(),
            &[Event::Created { id, user: 0 }]
        );
        let mut parcel = Parcel::new();
        aidl::OpenSession { session_id: id }.write(&mut parcel);
        let reply = call(&endpoint, aidl::OPEN_SESSION, 10101, &parcel);
        assert!(
            aidl::read_open_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .is_err()
        );
        assert!(!data.0.join(format!("app/vmdl{id}.tmp")).exists());
        let reply = call(&endpoint, aidl::OPEN_SESSION, 10100, &parcel);
        assert!(
            aidl::read_open_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap()
                .is_some()
        );
        let stage = data.0.join(format!("app/vmdl{id}.tmp"));
        assert!(stage.is_dir());
        assert_eq!(
            aim_storage::guest_inode::read(&stage)
                .unwrap()
                .unwrap()
                .mode,
            Some(0o775)
        );
        std::fs::write(
            stage.join("base.apk"),
            b"stage inventory only; commit is unsupported",
        )
        .unwrap();
        let mut parcel = Parcel::new();
        aidl::GetSessionInfo { session_id: id }.write(&mut parcel);
        let reply = call(&endpoint, aidl::GET_SESSION_INFO, 1000, &parcel);
        let info = aidl::read_get_session_info_reply::<super::super::codec::SessionInfo>(
            &mut Reader::new(reply.data(), reply.objects()),
        )
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(
            info.resolved_base_code_path,
            Some(format!("/data/app/vmdl{id}.tmp/base.apk"))
        );
        assert_eq!(info.installer_uid, 10100);
        drop(endpoint);
        drop(owner);
        drop(sessions);
        let recovered = Arc::new(Sessions::default());
        let owner = make_owner(&data, recovered.clone(), nodes, events);
        let endpoint = super::super::endpoint::Endpoint {
            sessions: recovered.clone(),
            owners: owner,
        };
        assert!(recovered.snapshot(id).unwrap().prepared);
        assert_eq!(recovered.snapshot(id).unwrap().active_count, 0);
        let mut parcel = Parcel::new();
        aidl::AbandonSession { session_id: id }.write(&mut parcel);
        let reply = call(&endpoint, aidl::ABANDON_SESSION, 10100, &parcel);
        assert!(
            aidl::read_abandon_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .is_ok()
        );
        assert!(!stage.exists());
        assert!(disk(&data).recovered().unwrap().is_empty());
    }
    #[test]
    fn caller_cannot_claim_other_installer_or_use_partial_cross_user_permission() {
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let owner = make_owner(
            &data,
            sessions.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        );
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner,
        };
        for (installer, user) in [("other", 0), ("fixture", 10)] {
            let mut parcel = Parcel::new();
            aidl::CreateSession {
                params: Some(request()),
                installer_package_name: Some(installer.into()),
                installer_attribution_tag: None,
                user_id: user,
            }
            .write(&mut parcel);
            let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
            assert_eq!(
                aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                    .unwrap()
                    .unwrap_err()
                    .code,
                aim_binder_host::parcel::EX_SECURITY
            );
            assert!(sessions.records().is_empty());
        }
        assert!(!data.0.join("system/install_sessions.xml").exists());
    }
    #[test]
    fn every_create_reads_current_user_policy_and_owned_oneoff_is_not_rearmed() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let denied = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicUsize::new(0));
        let flag = denied.clone();
        let count = reads.clone();
        let owner = make_owner_with_policy(
            &data,
            sessions.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(move |uid, user| {
                assert_eq!((uid, user), (10100, 0));
                count.fetch_add(1, Ordering::SeqCst);
                let mut policy = device();
                policy.users.get_mut(&user).unwrap().disallow_install_apps =
                    flag.load(Ordering::SeqCst);
                Ok(policy)
            }),
        );
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut parcel = Parcel::new();
        aidl::DisableVerificationForUid { uid: 10100 }.write(&mut parcel);
        let reply = call(
            &endpoint,
            aidl::DISABLE_VERIFICATION_FOR_UID,
            10100,
            &parcel,
        );
        assert!(
            aidl::read_disable_verification_for_uid_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_err()
        );
        let reply = call(&endpoint, aidl::DISABLE_VERIFICATION_FOR_UID, 1000, &parcel);
        assert!(
            aidl::read_disable_verification_for_uid_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_ok()
        );
        let mut parcel = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }
        .write(&mut parcel);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        let first =
            aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap();
        assert!(sessions.snapshot(first).unwrap().parameters.install_flags & 0x80000 != 0);
        denied.store(true, Ordering::SeqCst);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        assert_eq!(
            aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_SECURITY
        );
        assert_eq!(sessions.records().len(), 1);
        denied.store(false, Ordering::SeqCst);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        let second =
            aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap();
        assert_eq!(
            sessions.snapshot(second).unwrap().parameters.install_flags & 0x80000,
            0
        );
        assert_eq!(reads.load(Ordering::SeqCst), 3);
        owner.shutdown_callbacks();
        drop(endpoint);
        drop(owner);
        drop(worker);
    }
    #[test]
    fn create_retains_its_query_snapshot_while_live_policy_publishes_a_new_generation() {
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let current = Arc::new(Mutex::new(state()));
        let source = current.clone();
        let policy_state = current.clone();
        let owner = make_owner_with_source(
            &data,
            sessions.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(move |_, _| {
                let mut next = (**policy_state.lock().unwrap()).clone();
                next.generation += 1;
                // Keep the caller UID modelled while withdrawing its ownership
                // of the requested installer name. Removing its only UID entry
                // would test a missing permission owner instead of a denial.
                if let Some(mut caller) = next.packages.remove("fixture") {
                    caller.name = "other.installer".into();
                    Arc::make_mut(caller.pkg.as_mut().unwrap()).package_name = caller.name.clone();
                    next.packages.insert(caller.name.clone(), caller);
                }
                *policy_state.lock().unwrap() = Arc::new(next);
                Ok(device())
            }),
            Arc::new(move || Ok(source.lock().unwrap().clone())),
        );
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut parcel = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }.write(&mut parcel);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        let id = aidl::read_create_session_reply(&mut reply.reader()).unwrap().unwrap();
        assert_eq!(sessions.snapshot(id).unwrap().installer_uid, 10100);
        assert!(!current.lock().unwrap().packages.contains_key("fixture"));
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        assert_eq!(aidl::read_create_session_reply(&mut reply.reader()).unwrap().unwrap_err().code,
            aim_binder_host::parcel::EX_SECURITY);
        assert_eq!(sessions.records().len(), 1);
        owner.shutdown_callbacks();
        drop(endpoint);
        drop(owner);
        drop(worker);
    }
    #[test]
    fn callback_endpoint_rejects_trailing_registration_before_retaining_node() {
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let driver = aim_binder_driver::Driver::new();
        let process = LocalProcess::open(
            &driver,
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: 77901,
                euid: 1000,
                security_context: None,
            },
        );
        let owner = NativeOwners::open(
            sessions.clone(),
            Arc::new(|| Ok(state())),
            Arc::new(|_, _| panic!("callback registration must not capture install device policy")),
            SystemConfig::default(),
            disk(&data),
            Arc::new(|_| panic!("registration does not publish a session")),
            Arc::new(|_| panic!("registration emits no session event")),
            process.clone(),
        )
        .unwrap();
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions,
            owners: owner.clone(),
        };
        struct Callback;
        impl Service for Callback {
            fn descriptor(&self) -> &str {
                "android.content.pm.IPackageInstallerCallback"
            }
            fn transact(&self, _: &mut Call<'_>) -> aim_binder_host::local::Reply {
                panic!("no callback queued")
            }
        }
        let binder = process.add_service(Arc::new(Callback));
        let mut parcel = Parcel::new();
        aidl::RegisterCallback {
            callback: Some(binder),
            user_id: 0,
        }
        .write(&mut parcel);
        parcel.write_i32(1);
        assert_eq!(
            endpoint
                .transact(&mut Call {
                    code: aidl::REGISTER_CALLBACK,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: 10100,
                    data: Reader::new(parcel.data(), parcel.objects())
                })
                .unwrap_err(),
            aim_binder_host::parcel::BAD_VALUE
        );
        assert_eq!(owner.callbacks.len(), 0);
        let mut invalid = Parcel::new();
        aidl::RegisterCallback {
            callback: Some(binder),
            user_id: -1,
        }
        .write(&mut invalid);
        let reply = call(&endpoint, aidl::REGISTER_CALLBACK, 1000, &invalid);
        assert_eq!(
            aidl::read_register_callback_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
        );
        assert_eq!(owner.callbacks.len(), 0);
        let mut parcel = Parcel::new();
        aidl::RegisterCallback {
            callback: Some(binder),
            user_id: 0,
        }
        .write(&mut parcel);
        let reply = call(&endpoint, aidl::REGISTER_CALLBACK, 10100, &parcel);
        assert!(
            aidl::read_register_callback_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .is_ok()
        );
        assert_eq!(owner.callbacks.len(), 1);
        let mut parcel = Parcel::new();
        aidl::UnregisterCallback {
            callback: Some(binder),
        }
        .write(&mut parcel);
        let reply = call(&endpoint, aidl::UNREGISTER_CALLBACK, 10100, &parcel);
        assert!(
            aidl::read_unregister_callback_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .is_ok()
        );
        assert_eq!(owner.callbacks.len(), 0);
        owner.shutdown_callbacks();
        drop(endpoint);
        drop(owner);
        drop(worker);
        driver.release(process.proc_handle());
    }
    #[test]
    fn binder_label_change_persists_without_trimming_and_noop_emits_no_badging() {
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let events = Arc::new(Mutex::new(Vec::new()));
        let owner = make_owner(
            &data,
            sessions.clone(),
            Arc::new(Mutex::new(Vec::new())),
            events.clone(),
        );
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut parcel = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }
        .write(&mut parcel);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        let id = aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        let original = std::fs::read(data.0.join("system/install_sessions.xml")).unwrap();
        for (uid, label, expected) in [
            (
                10101,
                Some("other".into()),
                aim_binder_host::parcel::EX_SECURITY,
            ),
            (10100, None, aim_binder_host::parcel::EX_NULL_POINTER),
        ] {
            let mut parcel = Parcel::new();
            aidl::UpdateSessionAppLabel {
                session_id: id,
                app_label: label,
            }
            .write(&mut parcel);
            let reply = call(&endpoint, aidl::UPDATE_SESSION_APP_LABEL, uid, &parcel);
            assert_eq!(
                aidl::read_update_session_app_label_reply(&mut Reader::new(
                    reply.data(),
                    reply.objects()
                ))
                .unwrap()
                .unwrap_err()
                .code,
                expected
            );
            assert_eq!(
                std::fs::read(data.0.join("system/install_sessions.xml")).unwrap(),
                original
            );
        }
        let label = "<label & 😀>".repeat(120);
        let mut parcel = Parcel::new();
        aidl::UpdateSessionAppLabel {
            session_id: id,
            app_label: Some(label.clone()),
        }
        .write(&mut parcel);
        parcel.write_i32(123);
        assert_eq!(
            endpoint
                .transact(&mut Call {
                    code: aidl::UPDATE_SESSION_APP_LABEL,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: 10100,
                    data: Reader::new(parcel.data(), parcel.objects())
                })
                .unwrap_err(),
            aim_binder_host::parcel::BAD_VALUE
        );
        assert_eq!(
            std::fs::read(data.0.join("system/install_sessions.xml")).unwrap(),
            original
        );
        let mut parcel = Parcel::new();
        aidl::UpdateSessionAppLabel {
            session_id: id,
            app_label: Some(label.clone()),
        }
        .write(&mut parcel);
        for _ in 0..2 {
            let reply = call(&endpoint, aidl::UPDATE_SESSION_APP_LABEL, 10100, &parcel);
            assert!(
                aidl::read_update_session_app_label_reply(&mut Reader::new(
                    reply.data(),
                    reply.objects()
                ))
                .unwrap()
                .is_ok()
            );
        }
        assert_eq!(
            events.lock().unwrap().as_slice(),
            &[
                Event::Created { id, user: 0 },
                Event::Badging { id, user: 0 }
            ]
        );
        let records = disk(&data).recovered().unwrap();
        assert_eq!(
            records[0].1.params.app_label.as_deref(),
            Some(label.as_str())
        );
        owner.shutdown_callbacks();
        drop(endpoint);
        drop(owner);
        drop(worker);
    }
    #[test]
    fn binder_silent_policy_setters_enforce_identity_and_update_real_tracker() {
        let data = Data::new();
        let owner = make_owner(
            &data,
            Arc::new(Sessions::default()),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        );
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: owner.sessions.clone(),
            owners: owner.clone(),
        };
        owner.track_silent_update(Some("installer"), "app", 40_000);
        let mut parcel = Parcel::new();
        aidl::SetAllowUnlimitedSilentUpdates {
            installer_package_name: None,
        }
        .write(&mut parcel);
        let reply = call(
            &endpoint,
            aidl::SET_ALLOW_UNLIMITED_SILENT_UPDATES,
            10100,
            &parcel,
        );
        assert!(
            aidl::read_set_allow_unlimited_silent_updates_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_err()
        );
        assert!(!owner.silent_update_allowed(Some("installer"), "app", 50_000));
        let reply = call(
            &endpoint,
            aidl::SET_ALLOW_UNLIMITED_SILENT_UPDATES,
            1000,
            &parcel,
        );
        assert!(
            aidl::read_set_allow_unlimited_silent_updates_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_ok()
        );
        assert!(owner.silent_update_allowed(Some("installer"), "app", 50_000));
        owner.track_silent_update(Some("installer"), "app", 50_000);
        let mut parcel = Parcel::new();
        aidl::SetSilentUpdatesThrottleTime {
            throttle_time_in_seconds: 0,
        }
        .write(&mut parcel);
        parcel.write_i32(1);
        assert_eq!(
            endpoint
                .transact(&mut Call {
                    code: aidl::SET_SILENT_UPDATES_THROTTLE_TIME,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: 1000,
                    data: Reader::new(parcel.data(), parcel.objects())
                })
                .unwrap_err(),
            aim_binder_host::parcel::BAD_VALUE
        );
        assert!(!owner.silent_update_allowed(Some("installer"), "app", 50_001));
        let mut parcel = Parcel::new();
        aidl::SetSilentUpdatesThrottleTime {
            throttle_time_in_seconds: 0,
        }
        .write(&mut parcel);
        let reply = call(
            &endpoint,
            aidl::SET_SILENT_UPDATES_THROTTLE_TIME,
            2000,
            &parcel,
        );
        assert!(
            aidl::read_set_silent_updates_throttle_time_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_ok()
        );
        assert!(owner.silent_update_allowed(Some("installer"), "app", 50_001));
        assert!(!owner.silent_update_allowed(Some("installer"), "app", 50_000));
        owner.shutdown_callbacks();
        drop(endpoint);
        drop(owner);
        drop(worker);
    }
    #[test]
    fn session_names_read_fd_split_marker_loader_query_and_abandon_use_real_owner() {
        use aim_service_aidl::android_content_pm_ipackageinstallersession as session_api;
        use std::io::Read;
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let owner = make_owner(
            &data,
            sessions.clone(),
            nodes.clone(),
            Arc::new(Mutex::new(Vec::new())),
        );
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut p = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }
        .write(&mut p);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &p);
        let id = aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        let mut p = Parcel::new();
        aidl::OpenSession { session_id: id }.write(&mut p);
        call(&endpoint, aidl::OPEN_SESSION, 10100, &p);
        let node = nodes.lock().unwrap()[0].clone();
        let stage = data.0.join(format!("app/vmdl{id}.tmp"));
        std::fs::write(stage.join("base.apk"), b"real read capability").unwrap();
        std::fs::write(stage.join("app.metadata"), b"private metadata").unwrap();
        let mut p = Parcel::new();
        session_api::GetNames {}.write(&mut p);
        let reply = call(node.as_ref(), session_api::GET_NAMES, 10100, &p);
        assert_eq!(
            session_api::read_get_names_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap(),
            Some(vec![Some("base.apk".into())])
        );
        let mut p = Parcel::new();
        session_api::OpenRead {
            name: Some("base.apk".into()),
        }
        .write(&mut p);
        let reply = call(node.as_ref(), session_api::OPEN_READ, 10100, &p);
        let mut reader = Reader::new(reply.data(), reply.objects());
        reader.read_exception().unwrap().unwrap();
        assert_eq!(reader.read_i32().unwrap(), 1);
        assert_eq!(reader.read_i32().unwrap(), 0);
        reader.read_fd().unwrap();
        assert_eq!(reader.remaining(), 0);
        let fd = aim_binder_host::server::file_fd(&reply.files()[0].1).unwrap();
        let mut file = std::fs::File::from(fd);
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"real read capability");
        use std::os::fd::AsRawFd;
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) } & libc::O_ACCMODE,
            libc::O_RDONLY
        );
        let owned_target = data.0.join("outside-stage");
        std::fs::write(&owned_target, b"owned fixture only").unwrap();
        std::os::unix::fs::symlink(&owned_target, stage.join("escape.apk")).unwrap();
        let mut p = Parcel::new();
        session_api::OpenRead {
            name: Some("escape.apk".into()),
        }
        .write(&mut p);
        let reply = call(node.as_ref(), session_api::OPEN_READ, 10100, &p);
        assert!(
            Reader::new(reply.data(), reply.objects())
                .read_exception()
                .unwrap()
                .is_err()
        );
        assert!(reply.files().is_empty());
        let mut p = Parcel::new();
        session_api::RemoveSplit {
            split_name: Some("feature".into()),
        }
        .write(&mut p);
        call(node.as_ref(), session_api::REMOVE_SPLIT, 10100, &p).data();
        let marker = stage.join("feature.removed");
        assert!(marker.is_file());
        assert_eq!(
            aim_storage::guest_inode::read(&marker)
                .unwrap()
                .unwrap()
                .mode,
            Some(0)
        );
        let mut p = Parcel::new();
        session_api::OpenRead {
            name: Some("feature.removed".into()),
        }
        .write(&mut p);
        let reply = call(node.as_ref(), session_api::OPEN_READ, 10100, &p);
        assert!(
            Reader::new(reply.data(), reply.objects())
                .read_exception()
                .unwrap()
                .is_err()
        );
        assert!(reply.files().is_empty());
        let mut p = Parcel::new();
        session_api::GetDataLoaderParams {}.write(&mut p);
        let reply = call(node.as_ref(), session_api::GET_DATA_LOADER_PARAMS, 1000, &p);
        let mut reader = Reader::new(reply.data(), reply.objects());
        reader.read_exception().unwrap().unwrap();
        assert_eq!(reader.read_i32().unwrap(), 0);
        assert_eq!(reader.remaining(), 0);
        let mut p = Parcel::new();
        session_api::Abandon {}.write(&mut p);
        let reply = call(node.as_ref(), session_api::ABANDON, 10100, &p);
        session_api::read_abandon_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        assert!(!stage.exists());
        assert!(disk(&data).recovered().unwrap().is_empty());
        owner.shutdown_callbacks();
        nodes.lock().unwrap().clear();
        drop(node);
        drop(endpoint);
        drop(owner);
        drop(worker);
    }
    #[test]
    fn child_relationship_binder_changes_survive_file_owner_recovery() {
        use aim_service_aidl::android_content_pm_ipackageinstallersession as session_api;
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let owner = make_owner(
            &data,
            sessions.clone(),
            nodes.clone(),
            Arc::new(Mutex::new(Vec::new())),
        );
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let create = |multi| {
            let mut params = request();
            params.multi_package = multi;
            let mut p = Parcel::new();
            aidl::CreateSession {
                params: Some(params),
                installer_package_name: Some("fixture".into()),
                installer_attribution_tag: None,
                user_id: 0,
            }
            .write(&mut p);
            let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &p);
            aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap()
        };
        let parent = create(true);
        let child = create(false);
        let mut p = Parcel::new();
        aidl::OpenSession { session_id: parent }.write(&mut p);
        call(&endpoint, aidl::OPEN_SESSION, 10100, &p);
        let node = nodes.lock().unwrap()[0].clone();
        let mut p = Parcel::new();
        session_api::AddChildSessionId { session_id: child }.write(&mut p);
        let reply = call(node.as_ref(), session_api::ADD_CHILD_SESSION_ID, 10100, &p);
        reply.data();
        let recovered = disk(&data).recovered().unwrap();
        assert_eq!(
            recovered
                .iter()
                .find(|(s, _)| s.id == child)
                .unwrap()
                .0
                .parent,
            parent
        );
        assert!(
            recovered
                .iter()
                .find(|(s, _)| s.id == parent)
                .unwrap()
                .0
                .children
                .contains(&child)
        );
        let mut p = Parcel::new();
        session_api::RemoveChildSessionId { session_id: child }.write(&mut p);
        call(
            node.as_ref(),
            session_api::REMOVE_CHILD_SESSION_ID,
            10100,
            &p,
        );
        let recovered = disk(&data).recovered().unwrap();
        assert_eq!(
            recovered
                .iter()
                .find(|(s, _)| s.id == child)
                .unwrap()
                .0
                .parent,
            -1
        );
        assert!(
            recovered
                .iter()
                .find(|(s, _)| s.id == parent)
                .unwrap()
                .0
                .children
                .is_empty()
        );

        let mut p = Parcel::new();
        aidl::OpenSession { session_id: child }.write(&mut p);
        call(&endpoint, aidl::OPEN_SESSION, 10100, &p);
        let mut p = Parcel::new();
        session_api::AddChildSessionId { session_id: child }.write(&mut p);
        let reply = call(node.as_ref(), session_api::ADD_CHILD_SESSION_ID, 10100, &p);
        session_api::read_add_child_session_id_reply(&mut Reader::new(
            reply.data(),
            reply.objects(),
        ))
        .unwrap()
        .unwrap();
        let mut p = Parcel::new();
        session_api::Seal {}.write(&mut p);
        let reply = call(node.as_ref(), session_api::SEAL, 10100, &p);
        session_api::read_seal_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        let recovered = disk(&data).recovered().unwrap();
        assert!(recovered.iter().all(|(s, _)| s.sealed));
        let mut p = Parcel::new();
        session_api::RemoveChildSessionId { session_id: child }.write(&mut p);
        let reply = call(
            node.as_ref(),
            session_api::REMOVE_CHILD_SESSION_ID,
            10100,
            &p,
        );
        assert!(
            session_api::read_remove_child_session_id_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_err()
        );
        assert_eq!(sessions.snapshot(child).unwrap().parent, parent);
        owner.shutdown_callbacks();
        nodes.lock().unwrap().clear();
        drop(node);
        drop(endpoint);
        drop(owner);
        drop(worker);
    }
    #[test]
    fn transfer_seals_and_moves_actual_installer_authority_and_persisted_source() {
        use aim_service_aidl::android_content_pm_ipackageinstallersession as session_api;
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let mut captured = (*state()).clone();
        Arc::make_mut(
            captured
                .packages
                .get_mut("fixture")
                .unwrap()
                .pkg
                .as_mut()
                .unwrap(),
        )
        .queries_packages
        .push("new.installer".into());
        captured.packages.insert(
            "new.installer".into(),
            crate::package::model::PackageState {
                name: "new.installer".into(),
                app_id: 10101,
                pkg: Some(Arc::new(crate::package::pkg::AndroidPackage {
                    package_name: "new.installer".into(),
                    uid: 10101,
                    target_sdk_version: 35,
                    ..Default::default()
                })),
                users: [(
                    0,
                    crate::package::model::PackageUserState {
                        granted_permissions: vec!["android.permission.INSTALL_PACKAGES".into()],
                        ..Default::default()
                    },
                )]
                .into(),
                ..Default::default()
            },
        );
        let captured = Arc::new(captured);
        let owner = make_owner_with_source(
            &data,
            sessions.clone(),
            nodes.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(|_, _| Ok(device())),
            Arc::new(move || Ok(captured.clone())),
        );
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut p = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: Some("old tag".into()),
            user_id: 0,
        }
        .write(&mut p);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &p);
        let id = aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        let mut p = Parcel::new();
        aidl::OpenSession { session_id: id }.write(&mut p);
        call(&endpoint, aidl::OPEN_SESSION, 10100, &p);
        let node = nodes.lock().unwrap()[0].clone();
        let mut p = Parcel::new();
        session_api::Transfer {
            package_name: Some("new.installer".into()),
        }
        .write(&mut p);
        let reply = call(node.as_ref(), session_api::TRANSFER, 10100, &p);
        session_api::read_transfer_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        let stored = disk(&data).recovered().unwrap();
        assert_eq!(stored[0].0.installer_uid, 10101);
        assert!(stored[0].0.sealed);
        assert_eq!(
            stored[0].1.installer_package.as_deref(),
            Some("new.installer")
        );
        assert_eq!(
            stored[0].1.initiating_package.as_deref(),
            Some("new.installer")
        );
        assert_eq!(stored[0].1.installer_attribution_tag, None);
        let mut p = Parcel::new();
        session_api::GetNames {}.write(&mut p);
        let reply = call(node.as_ref(), session_api::GET_NAMES, 10100, &p);
        assert!(
            session_api::read_get_names_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .is_err()
        );
        let reply = call(node.as_ref(), session_api::GET_NAMES, 10101, &p);
        assert_eq!(
            session_api::read_get_names_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap(),
            Some(vec![])
        );
        owner.shutdown_callbacks();
        nodes.lock().unwrap().clear();
        drop(node);
        drop(endpoint);
        drop(owner);
        drop(worker);
    }
    struct OutgoingFd(aim_binder_driver::File);
    impl aim_service_aidl::WriteParcelable for OutgoingFd {
        fn write_to(&self, parcel: &mut Parcel) {
            parcel.write_i32(0);
            parcel.write_file(self.0.clone());
        }
    }
    #[test]
    fn actual_filebridge_open_write_preserves_offset_allocation_and_seal_lifetime() {
        use aim_service_aidl::android_content_pm_ipackageinstallersession as session_api;
        use std::io::{Read, Write};
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let process = LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: 77702,
                euid: 0,
                security_context: None,
            },
        );
        let published = nodes.clone();
        let owner = NativeOwners::open(
            sessions.clone(),
            Arc::new(|| Ok(state())),
            Arc::new(|_, _| Ok(device())),
            SystemConfig::default(),
            disk(&data),
            Arc::new(move |node| {
                let mut nodes = published.lock().unwrap();
                nodes.push(node);
                Ok(Binder::Local(nodes.len() as u64))
            }),
            Arc::new(|_| {}),
            process.clone(),
        )
        .unwrap();
        let callback_worker = owner.take_callback_worker().unwrap();
        let allocated = Arc::new(Mutex::new(Vec::new()));
        let record = allocated.clone();
        owner
            .configure_writer(
                Arc::new(|| Ok(false)),
                Arc::new(move |file, length, flags| {
                    record.lock().unwrap().push((length, flags));
                    file.set_len(length as u64)
                        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e.to_string()))
                }),
            )
            .unwrap();
        let io_guard = owner.take_io_worker_guard().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut p = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }
        .write(&mut p);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &p);
        let id = aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        let mut p = Parcel::new();
        aidl::OpenSession { session_id: id }.write(&mut p);
        call(&endpoint, aidl::OPEN_SESSION, 10100, &p);
        let node = nodes.lock().unwrap()[0].clone();
        std::fs::write(
            data.0.join(format!("app/vmdl{id}.tmp/base.apk")),
            b"prefix.........",
        )
        .unwrap();
        let mut p = Parcel::new();
        session_api::OpenWrite {
            name: Some("base.apk".into()),
            offset_bytes: 6,
            length_bytes: 15,
        }
        .write(&mut p);
        let reply = call(node.as_ref(), session_api::OPEN_WRITE, 10100, &p);
        Reader::new(reply.data(), reply.objects())
            .read_exception()
            .unwrap()
            .unwrap();
        let fd = aim_binder_host::server::file_fd(&reply.files()[0].1).unwrap();
        let mut stream = std::os::unix::net::UnixStream::from(fd);
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        assert!(owner.writes.open(id));
        let mut header = [0u8; 8];
        header[..4].copy_from_slice(&1i32.to_be_bytes());
        header[4..].copy_from_slice(&3i32.to_be_bytes());
        stream.write_all(&header).unwrap();
        stream.write_all(b"APK").unwrap();
        header[..4].copy_from_slice(&2i32.to_be_bytes());
        stream.write_all(&header).unwrap();
        let mut ack = [0; 8];
        stream.read_exact(&mut ack).unwrap();
        assert_eq!(ack, header);
        header[..4].copy_from_slice(&3i32.to_be_bytes());
        stream.write_all(&header).unwrap();
        stream.read_exact(&mut ack).unwrap();
        assert_eq!(ack, header);
        assert!(!owner.writes.open(id));
        assert_eq!(
            &std::fs::read(data.0.join(format!("app/vmdl{id}.tmp/base.apk"))).unwrap()[..9],
            b"prefixAPK"
        );
        assert_eq!(allocated.lock().unwrap()[0].0, 15);
        // Reverse copy receives a real capability through the local Binder driver.
        let input_path = data.0.join("incoming.apk");
        std::fs::write(&input_path, b"reverse-payload").unwrap();
        let incoming = std::fs::File::open(&input_path).unwrap();
        use std::os::fd::AsFd;
        let capability = aim_binder_host::server::file_from_fd(incoming.as_fd()).unwrap();
        let Binder::Local(ptr) = process.add_service(node.clone()) else {
            unreachable!()
        };
        let local = process.local_service(ptr).unwrap();
        let mut p = Parcel::new();
        session_api::Write {
            name: Some("reverse.apk".into()),
            offset_bytes: 0,
            length_bytes: 7,
            fd: Some(OutgoingFd(capability)),
        }
        .write(&mut p);
        let reply = local.transact(session_api::WRITE, &p, false).unwrap();
        reply.reader().read_exception().unwrap().unwrap();
        assert_eq!(
            std::fs::read(data.0.join(format!("app/vmdl{id}.tmp/reverse.apk"))).unwrap(),
            b"reverse"
        );
        assert!(!owner.writes.open(id));
        let mut p = Parcel::new();
        session_api::Write {
            name: Some("negative.apk".into()),
            offset_bytes: 0,
            length_bytes: -1,
            fd: Some(OutgoingFd(
                aim_binder_host::server::file_from_fd(incoming.as_fd()).unwrap(),
            )),
        }
        .write(&mut p);
        local
            .transact(session_api::WRITE, &p, false)
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::metadata(data.0.join(format!("app/vmdl{id}.tmp/negative.apk")))
                .unwrap()
                .len(),
            0
        );
        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        use std::os::fd::{FromRawFd, OwnedFd};
        let pipe_read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
        let _pipe_write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
        let mut p = Parcel::new();
        session_api::Write {
            name: Some("pipe.apk".into()),
            offset_bytes: 0,
            length_bytes: -1,
            fd: Some(OutgoingFd(
                aim_binder_host::server::file_from_fd(pipe_read.as_fd()).unwrap(),
            )),
        }
        .write(&mut p);
        let failed = local.transact(session_api::WRITE, &p, false).unwrap();
        let exception = failed.reader().read_exception().unwrap().unwrap_err();
        assert_eq!(exception.code, aim_binder_host::parcel::EX_PARCELABLE);
        assert_eq!(
            exception.message,
            "java.io.IOException: splice failed: EINVAL (Invalid argument)"
        );
        let mut p = Parcel::new();
        session_api::Seal {}.write(&mut p);
        let reply = call(node.as_ref(), session_api::SEAL, 10100, &p);
        session_api::read_seal_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        assert!(sessions.snapshot(id).unwrap().sealed);
        let mut p = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }
        .write(&mut p);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &p);
        let active_id =
            aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap();
        let mut p = Parcel::new();
        aidl::OpenSession {
            session_id: active_id,
        }
        .write(&mut p);
        call(&endpoint, aidl::OPEN_SESSION, 10100, &p);
        let active_node = nodes.lock().unwrap()[1].clone();
        let mut p = Parcel::new();
        session_api::OpenWrite {
            name: Some("base.apk".into()),
            offset_bytes: 0,
            length_bytes: -1,
        }
        .write(&mut p);
        let reply = call(active_node.as_ref(), session_api::OPEN_WRITE, 10100, &p);
        Reader::new(reply.data(), reply.objects())
            .read_exception()
            .unwrap()
            .unwrap();
        let fd = aim_binder_host::server::file_fd(&reply.files()[0].1).unwrap();
        let mut idle = std::os::unix::net::UnixStream::from(fd);
        idle.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        assert!(owner.writes.open(active_id));
        let mut p = Parcel::new();
        session_api::Seal {}.write(&mut p);
        let reply = call(active_node.as_ref(), session_api::SEAL, 10100, &p);
        assert!(
            Reader::new(reply.data(), reply.objects())
                .read_exception()
                .unwrap()
                .is_err()
        );
        assert!(sessions.snapshot(active_id).unwrap().destroyed);
        assert!(!owner.writes.open(active_id));
        assert!(!data.0.join(format!("app/vmdl{active_id}.tmp")).exists());
        assert_eq!(idle.read(&mut [0u8; 1]).unwrap(), 0);
        drop(active_node);
        owner.shutdown_callbacks();
        nodes.lock().unwrap().clear();
        drop(node);
        drop(endpoint);
        drop(owner);
        drop(io_guard);
        drop(callback_worker);
    }
    #[test]
    fn actual_revocable_capability_write_and_abandon_revoke_all_transferred_duplicates() {
        use aim_binder_host::proxy_file::{Operation, request as operation};
        use aim_service_aidl::android_content_pm_ipackageinstallersession as session_api;
        use std::os::fd::AsRawFd;
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let owner = make_owner(
            &data,
            sessions.clone(),
            nodes.clone(),
            Arc::new(Mutex::new(Vec::new())),
        );
        let callback = owner.take_callback_worker().unwrap();
        owner
            .configure_writer(
                Arc::new(|| Ok(true)),
                Arc::new(|file, length, _| {
                    file.set_len(length as u64)
                        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e.to_string()))
                }),
            )
            .unwrap();
        let io = owner.take_io_worker_guard().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut p = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }
        .write(&mut p);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &p);
        let id = aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        let mut p = Parcel::new();
        aidl::OpenSession { session_id: id }.write(&mut p);
        call(&endpoint, aidl::OPEN_SESSION, 10100, &p);
        let node = nodes.lock().unwrap()[0].clone();
        let mut p = Parcel::new();
        session_api::OpenWrite {
            name: Some("base.apk".into()),
            offset_bytes: 0,
            length_bytes: 5,
        }
        .write(&mut p);
        let reply = call(node.as_ref(), session_api::OPEN_WRITE, 10100, &p);
        Reader::new(reply.data(), reply.objects())
            .read_exception()
            .unwrap()
            .unwrap();
        let file = &reply.files()[0].1;
        assert_eq!(
            aim_binder_host::server::file_class(file),
            Some(aim_binder_host::proxy_file::CLASS)
        );
        let fd = aim_binder_host::server::file_fd(file).unwrap();
        let duplicate = fd.try_clone().unwrap();
        assert_eq!(
            operation(fd.as_raw_fd(), Operation::Write, 0, 5, b"APK!!")
                .unwrap()
                .0,
            5
        );
        operation(duplicate.as_raw_fd(), Operation::Sync, 0, 0, &[]).unwrap();
        assert_eq!(
            std::fs::read(data.0.join(format!("app/vmdl{id}.tmp/base.apk"))).unwrap(),
            b"APK!!"
        );
        assert!(owner.writes.open(id));
        let mut p = Parcel::new();
        session_api::Abandon {}.write(&mut p);
        let reply = call(node.as_ref(), session_api::ABANDON, 10100, &p);
        Reader::new(reply.data(), reply.objects())
            .read_exception()
            .unwrap()
            .unwrap();
        assert!(!owner.writes.open(id));
        assert!(!data.0.join(format!("app/vmdl{id}.tmp")).exists());
        for fd in [&fd, &duplicate] {
            assert_eq!(
                operation(fd.as_raw_fd(), Operation::Write, 0, 1, b"X")
                    .unwrap_err()
                    .raw_os_error(),
                Some(libc::EPERM)
            );
        }
        owner.shutdown_callbacks();
        nodes.lock().unwrap().clear();
        drop(node);
        drop(endpoint);
        drop(owner);
        drop(io);
        drop(callback);
    }
    #[test]
    fn app_metadata_binder_flags_real_mode_readback_removal_and_recovery_match_original() {
        use aim_service_aidl::android_content_pm_ipackageinstallersession as session_api;
        use std::io::{Read, Write};
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let owner = make_owner(
            &data,
            sessions.clone(),
            nodes.clone(),
            Arc::new(Mutex::new(Vec::new())),
        );
        let callback = owner.take_callback_worker().unwrap();
        owner
            .configure_writer(Arc::new(|| Ok(false)), Arc::new(|_, _, _| Ok(())))
            .unwrap();
        let io = owner.take_io_worker_guard().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut p = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }
        .write(&mut p);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &p);
        let id = aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        let mut p = Parcel::new();
        aidl::OpenSession { session_id: id }.write(&mut p);
        call(&endpoint, aidl::OPEN_SESSION, 10100, &p);
        let node = nodes.lock().unwrap()[0].clone();
        let mut get = Parcel::new();
        session_api::GetAppMetadataFd {}.write(&mut get);
        let reply = call(node.as_ref(), session_api::GET_APP_METADATA_FD, 10100, &get);
        let mut r = Reader::new(reply.data(), reply.objects());
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 0);
        let bad = call(node.as_ref(), session_api::GET_APP_METADATA_FD, 10101, &get);
        assert_eq!(
            Reader::new(bad.data(), bad.objects())
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_SECURITY
        );
        fn finish(file: &aim_binder_driver::File, bytes: &[u8]) {
            let fd = aim_binder_host::server::file_fd(file).unwrap();
            let mut stream = std::os::unix::net::UnixStream::from(fd);
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut header = [0u8; 8];
            header[..4].copy_from_slice(&1i32.to_be_bytes());
            header[4..].copy_from_slice(&(bytes.len() as i32).to_be_bytes());
            stream.write_all(&header).unwrap();
            stream.write_all(bytes).unwrap();
            header[..4].copy_from_slice(&3i32.to_be_bytes());
            stream.write_all(&header).unwrap();
            let mut ack = [0u8; 8];
            stream.read_exact(&mut ack).unwrap();
            assert_eq!(ack, header);
        }
        let mut ordinary = Parcel::new();
        session_api::OpenWrite {
            name: Some("app.metadata".into()),
            offset_bytes: 0,
            length_bytes: -1,
        }
        .write(&mut ordinary);
        let reply = call(node.as_ref(), session_api::OPEN_WRITE, 10100, &ordinary);
        finish(&reply.files()[0].1, b"ignored");
        assert!(!sessions.snapshot(id).unwrap().has_app_metadata);
        let mut p = Parcel::new();
        session_api::OpenWriteAppMetadata {}.write(&mut p);
        let reply = call(
            node.as_ref(),
            session_api::OPEN_WRITE_APP_METADATA,
            10100,
            &p,
        );
        Reader::new(reply.data(), reply.objects())
            .read_exception()
            .unwrap()
            .unwrap();
        finish(&reply.files()[0].1, b"JSON");
        assert!(sessions.snapshot(id).unwrap().has_app_metadata);
        let path = data.0.join(format!("app/vmdl{id}.tmp/app.metadata"));
        assert_eq!(
            aim_storage::guest_inode::read(&path).unwrap().unwrap().mode,
            Some(0o640)
        );
        let reply = call(node.as_ref(), session_api::GET_APP_METADATA_FD, 10100, &get);
        let mut file =
            std::fs::File::from(aim_binder_host::server::file_fd(&reply.files()[0].1).unwrap());
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "JSONred");
        assert!(
            !owner
                .disk
                .lock()
                .unwrap()
                .recovered()
                .unwrap()
                .iter()
                .find(|(session, _)| session.id == id)
                .unwrap()
                .0
                .has_app_metadata
        );
        let mut p = Parcel::new();
        session_api::RemoveAppMetadata {}.write(&mut p);
        p.write_i32(99);
        assert!(
            node.transact(&mut Call {
                code: session_api::REMOVE_APP_METADATA,
                flags: 0,
                sender_pid: 77,
                sender_euid: 10100,
                data: Reader::new(p.data(), p.objects())
            })
            .is_err()
        );
        assert!(path.exists());
        let mut p = Parcel::new();
        session_api::RemoveAppMetadata {}.write(&mut p);
        let reply = call(node.as_ref(), session_api::REMOVE_APP_METADATA, 10101, &p);
        Reader::new(reply.data(), reply.objects())
            .read_exception()
            .unwrap()
            .unwrap();
        assert!(!path.exists());
        assert!(!sessions.snapshot(id).unwrap().has_app_metadata);
        let reply = call(node.as_ref(), session_api::GET_APP_METADATA_FD, 10100, &get);
        let mut r = Reader::new(reply.data(), reply.objects());
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 0);
        owner.shutdown_callbacks();
        nodes.lock().unwrap().clear();
        drop(node);
        drop(endpoint);
        drop(owner);
        drop(io);
        drop(callback);
    }
}

impl super::SessionOperations for NativeOwners {
    fn commit_session(
        &self,
        id: i32,
        uid: u32,
        receiver: Option<super::preapproval::IntentSender>,
        transferred: bool,
    ) -> Result<(), Exception> {
        let captured = self.sessions.snapshot(id)?;
        if captured.parent != -1 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "commit cannot be called on a child session",
            ));
        }
        let receiver = receiver.ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "statusReceiver can't be null for the root session",
            )
        })?;
        let external = self.external.lock().unwrap().clone().ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "Installer status/policy owner unavailable",
            )
        })?;
        let policy = self.commit_policy.lock().unwrap().clone().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "Installer FRP/compat policy unavailable")
        })?(uid, receiver)?;
        if policy.require_mutable_receiver && external.immutable(receiver)? {
            return Err(Exception::illegal_argument(
                "The commit() status receiver should come from a mutable PendingIntent",
            ));
        }
        captured.owner(uid)?;
        if !captured.prepared || captured.destroyed {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Commit session is not prepared or destroyed",
            ));
        }
        if self.writes.open(id) {
            return Err(Exception::security("Files still open"));
        }
        if policy.secure_frp && !policy.secure_frp_install_allowed {
            return Err(Exception::security(
                "Can't install packages while in secure FRP",
            ));
        }
        if transferred {
            self.query(uid, |query| {
                if policy::permission(query, "android.permission.INSTALL_PACKAGES")? {
                    Ok(())
                } else {
                    Err(Exception::security("Requires INSTALL_PACKAGES"))
                }
            })?;
            if captured.original_installer_uid == captured.installer_uid {
                return Err(Exception::illegal_argument(
                    "Session has not been transferred",
                ));
            }
        } else if captured.original_installer_uid != captured.installer_uid {
            return Err(Exception::illegal_argument("Session has been transferred"));
        }
        let process = self
            .process
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Installer process closed"))?;
        let retained = match receiver.target {
            Binder::Handle(handle) => Some(process.strong(handle)),
            Binder::Local(pointer) => {
                process.local_service(pointer).ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, "Status receiver node unavailable")
                })?;
                None
            }
        };
        self.receivers
            .lock()
            .unwrap()
            .insert(id, (receiver, retained));
        self.seal_session(id, uid)?;
        let limit = external.metadata_limit()?;
        self.sessions.with_record_mut(id, |session, record| {
            if session.has_app_metadata {
                let file = self
                    .disk
                    .lock()
                    .unwrap()
                    .read_file(session, record, Some("app.metadata"))
                    .map_err(disk_error)?;
                if file
                    .metadata()
                    .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?
                    .len() as i64
                    > limit
                {
                    self.disk
                        .lock()
                        .unwrap()
                        .remove_metadata(session, record)
                        .map_err(disk_error)?;
                    session.has_app_metadata = false;
                    return Err(Exception::illegal_argument(format!(
                        "App metadata size exceeds the maximum allowed limit of {limit}"
                    )));
                }
            }
            Ok(())
        })?;
        self.persist()?;
        self.commit_queue
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Installer commit queue unavailable"))?
            .send(Some(id))
            .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "Installer commit worker stopped"))
    }
    fn request_preapproval(
        &self,
        id: i32,
        uid: u32,
        body: super::codec::Object,
    ) -> Result<(), Exception> {
        self.approval_owner()?
            .request(&self.approval_session(id)?, uid, &body)
            .map(|_| ())
    }
    fn stage_hard_link(&self, id: i32, uid: u32, target: Option<String>) -> Result<(), Exception> {
        if uid != 1000 {
            return Err(Exception::security("link() can only be run by the system"));
        }
        let stage = self.sessions.with_record(id, |session, record| {
            self.disk
                .lock()
                .unwrap()
                .validation_path(session, record)
                .map_err(disk_error)
        })?;
        self.files()?.stage(uid, target, stage)
    }
    fn set_checksums(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        checksums: Option<Vec<Option<super::checksums::Checksum>>>,
        signature: Option<Vec<u8>>,
    ) -> Result<(), Exception> {
        let checksums = checksums.ok_or_else(|| {
            Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null checksums")
        })?;
        if checksums.is_empty() {
            return Ok(());
        }
        let installer = self.sessions.with_record(id, |_, record| {
            super::checksums::installer_name(record)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Installer package is empty."))
        })?;
        self.check_package(uid, Some(&installer))?;
        self.query(1000, |query| {
            if !query
                .state
                .packages
                .get(&installer)
                .is_some_and(|package| package.pkg.is_some())
            {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Can't obtain calling installer's package.",
                ));
            }
            Ok(())
        })?;
        let files = self.files()?;
        let (captured, record) = self
            .sessions
            .records()
            .into_iter()
            .find(|(session, _)| session.id == id)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Unknown session"))?;
        let key = name.clone();
        let mut pending = captured.checksums.clone();
        pending.set(
            &captured,
            &record,
            uid,
            name,
            checksums,
            signature,
            &installer,
            captured.committed,
            &files,
        )?;
        let verified = pending
            .0
            .into_iter()
            .find(|(name, _)| name == &key)
            .expect("Verified nonempty checksums inserted their entry")
            .1;
        self.sessions.with_record_mut(id, |session, _| {
            session.owner(uid)?;
            if !session.prepared {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "addChecksums before prepared",
                ));
            }
            if session.destroyed || session.committed {
                return Err(Exception::security(
                    "addChecksums not allowed after commit or destruction",
                ));
            }
            if session.checksums.0.iter().any(|(name, _)| name == &key) {
                return Err(Exception::new(EX_ILLEGAL_STATE, "Duplicate checksums."));
            }
            let hash = key
                .as_deref()
                .map(crate::package::info::java_hash)
                .unwrap_or(0);
            let index = session
                .checksums
                .0
                .iter()
                .position(|(name, _)| {
                    name.as_deref()
                        .map(crate::package::info::java_hash)
                        .unwrap_or(0)
                        > hash
                })
                .unwrap_or(session.checksums.0.len());
            session.checksums.0.insert(index, (key, verified));
            Ok(())
        })
    }

    fn request_checksums(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        optional: i32,
        required: i32,
        trusted: super::checksums::TrustedInstallers,
        listener: Option<Binder>,
    ) -> Result<(), Exception> {
        let (session, record) = self
            .sessions
            .records()
            .into_iter()
            .find(|(session, _)| session.id == id)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Unknown session"))?;
        let verifier = session.sealed && self.is_verifier(uid)?;
        if uid != 0 && uid != session.installer_uid && !verifier {
            return Err(Exception::security("Session does not belong to caller"));
        }
        let stage = self
            .disk
            .lock()
            .unwrap()
            .validation_path(&session, &record)
            .map_err(disk_error)?;
        let name = name.ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "null checksum file name",
            )
        })?;
        super::checksums::Requests {
            files: self.files()?,
        }
        .request(
            &session,
            &record,
            uid,
            verifier,
            format!("{stage}/{name}"),
            optional,
            required,
            trusted,
            listener,
        )
    }
    fn fetch_package_names(&self, id: i32, uid: u32) -> Result<Vec<Option<String>>, Exception> {
        let session = self.sessions.snapshot(id)?;
        if session.parent != -1 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "fetchPackageNames not supported for child sessions",
            ));
        }
        session.owner(uid)?;
        let policy = self.lite_policy.lock().unwrap().clone().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                "Installer APKLite platform/property/file filter owner unavailable",
            )
        })?;
        let policy = policy()?;
        let ids: Vec<_> = if session.parameters.multi_package {
            session.children.iter().copied().collect()
        } else {
            vec![id]
        };
        ids.into_iter()
            .map(|id| {
                self.sessions.with_record(id, |session, record| {
                    if !session.sealed {
                        return Err(Exception::new(
                            EX_ILLEGAL_STATE,
                            "fetchPackageName before sealing",
                        ));
                    }
                    self.disk
                        .lock()
                        .unwrap()
                        .fetch_package_name(session, record, &policy)
                        .map(Some)
                        .map_err(|error| {
                            Exception::new(
                                EX_ILLEGAL_STATE,
                                format!("Can't parse package for session={id}: {}", error.message),
                            )
                        })
                })
            })
            .collect()
    }
    fn add_file(&self, id: i32, uid: u32, file: super::InstallationFile) -> Result<(), Exception> {
        self.query(uid, |query| {
            if !policy::permission(query, "android.permission.USE_INSTALLER_V2")? {
                return Err(Exception::security("Requires USE_INSTALLER_V2"));
            }
            Ok(())
        })?;
        self.sessions.with_record_mut(id, |session, record| {
            let loader = record.params.data_loader_params.as_ref().ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "Cannot add files to non-data loader installation session.",
                )
            })?;
            let loader = super::codec::DataLoader::from_object(loader)
                .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "Invalid data loader parcel"))?;
            if loader.kind == 1 && file.location != 0 {
                return Err(Exception::illegal_argument(format!(
                    "Non-incremental installation only supports /data/app placement: {}",
                    file.name.as_deref().unwrap_or("null")
                )));
            }
            if file.metadata.is_none() {
                return Err(Exception::illegal_argument(
                    "DataLoader installation requires valid metadata",
                ));
            }
            if !file.name.as_deref().is_some_and(storage::valid_filename) {
                return Err(Exception::illegal_argument("Invalid name"));
            }
            session.owner(uid)?;
            session.mutable()?;
            if session
                .installation_files
                .iter()
                .any(|existing| existing.location == file.location && existing.name == file.name)
            {
                return Err(Exception::illegal_argument("File already added"));
            }
            session.installation_files.push(file);
            Ok(())
        })
    }
    fn remove_file(
        &self,
        id: i32,
        uid: u32,
        location: i32,
        name: Option<String>,
    ) -> Result<(), Exception> {
        self.query(uid, |query| {
            if !policy::permission(query, "android.permission.USE_INSTALLER_V2")? {
                return Err(Exception::security("Requires USE_INSTALLER_V2"));
            }
            Ok(())
        })?;
        self.sessions.with_record_mut(id, |session, record| {
            if record.params.data_loader_params.is_none() {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Cannot add files to non-data loader installation session.",
                ));
            }
            if record
                .params
                .app_package_name
                .as_deref()
                .is_none_or(str::is_empty)
            {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Must specify package name to remove a split",
                ));
            }
            session.owner(uid)?;
            session.mutable()?;
            let name = format!("{}.removed", name.as_deref().unwrap_or("null"));
            if !storage::valid_filename(&name) {
                return Err(Exception::illegal_argument("Invalid marker"));
            }
            if session.installation_files.iter().any(|existing| {
                existing.location == location && existing.name.as_deref() == Some(&name)
            }) {
                return Err(Exception::illegal_argument("File already removed"));
            }
            session.installation_files.push(super::InstallationFile {
                location,
                name: Some(name),
                length: -1,
                metadata: None,
                signature: None,
            });
            Ok(())
        })
    }
    fn pre_verified_domains(&self, id: i32, uid: u32) -> Result<Option<Vec<String>>, Exception> {
        self.sessions.with_record(id, |session, _| {
            session.owner(uid)?;
            if !session.prepared {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "getPreVerifiedDomains before prepared",
                ));
            }
            if session.destroyed {
                return Err(Exception::security("Session is destroyed"));
            }
            Ok(session.pre_verified_domains.clone())
        })
    }
    fn set_pre_verified_domains(
        &self,
        id: i32,
        uid: u32,
        domains: Option<crate::package::domain_verification::domain_set::DomainSet>,
    ) -> Result<(), Exception> {
        let policy = self.domain_policy.lock().unwrap().clone().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                "Installer DeviceConfig and instant app installer owner unavailable",
            )
        })?;
        let session = self.sessions.snapshot(id)?;
        let (_, record) = self
            .sessions
            .records()
            .into_iter()
            .find(|(s, _)| s.id == id)
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "Session parameter owner unavailable")
            })?;
        let (count, length, installer) = policy()?;
        if session.installer_uid != 0 && session.installer_uid != 2000 {
            self.query(1000, |query| {
                if !query.uid_has_permission(session.installer_uid as i32, "android.permission.ACCESS_INSTANT_APPS").map_err(policy::unknown)? {
                    return Err(Exception::security("You need android.permission.ACCESS_INSTANT_APPS permission to set pre-verified domains."));
                }
                let installer = installer.as_ref().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Instant app installer is not available. Only the instant app installer can call this API."))?;
                if record.installer_package.as_ref() != Some(installer) { return Err(Exception::security("Only the instant app installer can call this API.")); }
                Ok(())
            })?;
        }
        let domains = domains.ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "null preVerifiedDomains",
            )
        })?;
        let domains = match domains {
            crate::package::domain_verification::domain_set::DomainSet::Hosts(hosts) => hosts,
            domains => {
                let process = self.process.upgrade().ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, "Installer process unavailable")
                })?;
                domains
                    .resolve(&process)
                    .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("DomainSet blob: {e}")))?
            }
        };
        if domains.len() as i64 > count {
            return Err(Exception::illegal_argument(format!(
                "The number of pre-verified domains have exceeded the maximum of {count}"
            )));
        }
        let mut values = Vec::new();
        for domain in domains {
            let domain = domain.ok_or_else(|| {
                Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null domain")
            })?;
            if domain.encode_utf16().count() as i64 > length {
                return Err(Exception::illegal_argument(format!(
                    "Pre-verified domain: [{domain} ] exceeds maximum length allowed: {length}"
                )));
            }
            values.push(domain);
        }
        self.sessions.with_record_mut(id, |session, _| {
            session.owner(uid)?;
            session.mutable()?;
            session.pre_verified_domains = Some(values);
            Ok(())
        })
    }
    fn metadata_read(
        &self,
        id: i32,
        uid: u32,
    ) -> Result<Option<aim_binder_driver::File>, Exception> {
        self.sessions.with_record(id, |session, record| {
            session.owner(uid)?;
            if !session.prepared || session.destroyed {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Session is not prepared or destroyed",
                ));
            }
            if !session.has_app_metadata {
                return Ok(None);
            }
            let file = self
                .disk
                .lock()
                .unwrap()
                .read_file(session, record, Some("app.metadata"))
                .map_err(disk_error)?;
            use std::os::fd::AsFd;
            aim_binder_host::server::file_from_fd(file.as_fd())
                .map(Some)
                .ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, "Cannot retain app metadata read FD")
                })
        })
    }
    fn metadata_write(&self, id: i32, uid: u32) -> Result<aim_binder_driver::File, Exception> {
        self.sessions.with_record(id, |session, _| {
            session.owner(uid)?;
            session.mutable()
        })?;
        let file = self.open_write(id, uid, Some("app.metadata".into()), 0, -1)?;
        self.sessions.with_record_mut(id, |session, _| {
            session.has_app_metadata = true;
            Ok(())
        })?;
        Ok(file)
    }
    fn metadata_remove(&self, id: i32) -> Result<(), Exception> {
        self.sessions.with_record_mut(id, |session, record| {
            if session.has_app_metadata {
                // Original File.delete() returns a boolean which this method
                // ignores; retain failed deletion evidence in the owner log.
                if let Err(error) = self.disk.lock().unwrap().remove_metadata(session, record) {
                    self.errors.lock().unwrap().push(format!(
                        "App metadata deletion returned false: {}",
                        error.message
                    ));
                }
                session.has_app_metadata = false;
            }
            Ok(())
        })
    }
    fn open_write(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        offset: i64,
        length: i64,
    ) -> Result<aim_binder_driver::File, Exception> {
        let (target, transfer, revocable) =
            self.prepare_write(id, uid, name, offset, length, false)?;
        if revocable {
            return self
                .writes
                .start_proxy(transfer, target)
                .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()));
        }
        let socket = self
            .writes
            .start(id, transfer, target)
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
        use std::os::fd::AsFd;
        aim_binder_host::server::file_from_fd(socket.as_fd())
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Cannot retain FileBridge socket"))
    }
    fn write_file(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        offset: i64,
        length: i64,
        fd: Option<u32>,
    ) -> Result<(), Exception> {
        let (mut target, transfer, _) = self.prepare_write(id, uid, name, offset, length, true)?;
        let result = (|| -> Result<(), Exception> {
            let fd = fd.ok_or_else(|| {
                Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null incoming FD")
            })?;
            let process = self.process.upgrade().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "installer Binder process unavailable")
            })?;
            let file = process.file(fd).ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "incoming FD capability unavailable")
            })?;
            let fd = aim_binder_host::server::file_fd(&file).ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "incoming FD fileport unavailable")
            })?;
            let mut incoming = std::fs::File::from(fd);
            use std::io::Read;
            let mut buffer = [0u8; 8192];
            if length < 0 {
                use std::os::unix::fs::FileTypeExt;
                if incoming
                    .metadata()
                    .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?
                    .file_type()
                    .is_fifo()
                {
                    let cause = "splice failed: EINVAL (Invalid argument)";
                    let mut payload = aim_binder_host::parcel::Parcel::new();
                    payload.write_string16(Some("android.os.ParcelableException"));
                    payload.write_string16(Some("java.io.IOException"));
                    payload.write_string16(Some(cause));
                    return Err(Exception::parcelable(
                        Some(&format!("java.io.IOException: {cause}")),
                        &payload,
                    )
                    .map_err(|_| {
                        Exception::new(EX_ILLEGAL_STATE, "Invalid IOException envelope")
                    })?);
                }
                // A negative sendfile count fails EINVAL, then FileUtils uses
                // SizedInputStream, whose nonpositive bound returns EOF. Its
                // socket copy branch also makes no iterations for this count.
            }
            let mut left = length.max(0) as u64;
            let mut delta = 0u64;
            let size = self
                .sessions
                .with_record(id, |_, r| Ok(r.params.size_bytes))?;
            let progress = |delta: u64| -> Result<(), Exception> {
                if size > 0 {
                    if let Some(event) =
                        self.sessions
                            .progress(id, uid, delta as f32 / size as f32, true)?
                    {
                        self.notify(event);
                    }
                }
                Ok(())
            };
            while left > 0 {
                let count = left.min(buffer.len() as u64) as usize;
                let n = incoming
                    .read(&mut buffer[..count])
                    .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
                if n == 0 {
                    break;
                }
                use std::io::Write;
                target
                    .write_all(&buffer[..n])
                    .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
                left -= n as u64;
                delta += n as u64;
                if delta >= 512 * 1024 {
                    progress(delta)?;
                    delta = 0;
                }
            }
            progress(delta)?;
            Ok(())
        })();
        drop(target);
        transfer.stop();
        result
    }

    fn names(&self, id: i32, uid: u32) -> Result<Vec<Option<String>>, Exception> {
        self.sessions.with_record(id, |session, record| {
            if uid != 0
                && uid != session.installer_uid
                && !(session.sealed && self.is_verifier(uid)?)
            {
                return Err(Exception::security(format!(
                    "Session does not belong to uid {uid}"
                )));
            }
            if !session.prepared || session.destroyed {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Session is not prepared or destroyed",
                ));
            }
            if record.params.data_loader_params.is_some() {
                return Ok(session
                    .installation_files
                    .iter()
                    .filter(|file| file.name.as_deref() != Some("app.metadata"))
                    .map(|file| file.name.clone())
                    .collect());
            }
            self.disk
                .lock()
                .unwrap()
                .names(session, record)
                .map_err(disk_error)
        })
    }
    fn open_read(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
    ) -> Result<aim_binder_driver::File, Exception> {
        self.sessions.with_record(id, |session, record| {
            if record.params.data_loader_params.is_some() {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Cannot read regular files in a data loader installation session.",
                ));
            }
            if uid != 0 && uid != session.installer_uid {
                return Err(Exception::security(format!(
                    "Session does not belong to uid {uid}"
                )));
            }
            if !session.prepared || session.destroyed {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Session is not prepared or destroyed",
                ));
            }
            if !name.as_deref().is_some_and(storage::valid_filename) {
                return Err(Exception::illegal_argument("Invalid name"));
            }
            let file = self
                .disk
                .lock()
                .unwrap()
                .read_file(session, record, name.as_deref())
                .map_err(disk_error)?;
            use std::os::fd::AsFd;
            aim_binder_host::server::file_from_fd(file.as_fd())
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Cannot retain session read FD"))
        })
    }
    fn remove_split(&self, id: i32, uid: u32, name: Option<String>) -> Result<(), Exception> {
        self.sessions.with_record(id, |session, record| {
            if record.params.data_loader_params.is_some() {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Cannot remove splits in a data loader installation session.",
                ));
            }
            if record
                .params
                .app_package_name
                .as_deref()
                .is_none_or(str::is_empty)
            {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Must specify package name to remove a split",
                ));
            }
            if uid != 0 && uid != session.installer_uid {
                return Err(Exception::security(format!(
                    "Session does not belong to uid {uid}"
                )));
            }
            if !session.prepared || session.destroyed {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Session is not prepared or destroyed",
                ));
            }
            let marker = format!("{}.removed", name.as_deref().unwrap_or("null"));
            if !storage::valid_filename(&marker) {
                return Err(Exception::illegal_argument(format!(
                    "Invalid marker: {marker}"
                )));
            }
            self.disk
                .lock()
                .unwrap()
                .remove_split(session, record, name.as_deref())
                .map_err(disk_error)
        })
    }
    fn abandon_session(&self, id: i32, uid: u32) -> Result<(), Exception> {
        let ids = self.sessions.abandon(id, uid)?;
        self.abandon_stage(&ids)
    }
    fn data_loader(&self, id: i32, uid: u32) -> Result<Option<super::codec::Object>, Exception> {
        self.query(uid, |query| {
            if !policy::permission(query, "android.permission.USE_INSTALLER_V2")? {
                return Err(Exception::security(
                    "getDataLoaderParams requires USE_INSTALLER_V2",
                ));
            }
            self.sessions
                .with_record(id, |_, record| Ok(record.params.data_loader_params.clone()))
        })
    }
    fn transfer_session(&self, id: i32, uid: u32, name: Option<String>) -> Result<(), Exception> {
        let name = name
            .filter(|name| !name.is_empty())
            .ok_or_else(|| Exception::illegal_argument("destination package cannot be empty"))?;
        let (_, record) = self
            .sessions
            .records()
            .into_iter()
            .find(|(session, _)| session.id == id)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Unknown session"))?;
        let new_uid = self.query(uid, |query| {
            let app = query
                .application_info(&name, 0, record.user as i32)
                .map_err(policy::unknown)??;
            let Some(app) = app else {
                let mut payload = aim_binder_host::parcel::Parcel::new();
                payload.write_string16(Some("android.os.ParcelableException"));
                payload.write_string16(Some(
                    "android.content.pm.PackageManager$NameNotFoundException",
                ));
                payload.write_string16(Some(&name));
                let message =
                    format!("android.content.pm.PackageManager$NameNotFoundException: {name}");
                return Err(
                    Exception::parcelable(Some(&message), &payload).map_err(|_| {
                        Exception::new(EX_ILLEGAL_STATE, "NameNotFoundException capability payload")
                    })?,
                );
            };
            if !query
                .uid_has_permission(app.uid, "android.permission.INSTALL_PACKAGES")
                .map_err(policy::unknown)?
            {
                return Err(Exception::security(format!(
                    "Destination package {name} does not have INSTALL_PACKAGES permission"
                )));
            }
            Ok(app.uid as u32)
        })?;
        let mask = 0x80 | 0x100000 | 0x1000 | 0x800 | 0x4000 | 0x10000 | 0x8000;
        if record.params.install_flags & mask == record.params.install_flags
            && record.params.abi_override.is_none()
            && record.params.volume_uuid.is_none()
        {
            return Err(Exception::security(
                "Can only transfer sessions that use public options",
            ));
        }
        self.sessions
            .transfer(id, uid, name, new_uid, &|id| self.writes.open(id))?;
        self.persist()
    }
    fn seal_session(&self, id: i32, uid: u32) -> Result<(), Exception> {
        match self.sessions.seal(id, uid, &|id| self.writes.open(id)) {
            Ok(_) => self.persist(),
            Err((failed, error)) => {
                if !failed.is_empty() {
                    self.abandon_stage(&failed)?;
                }
                Err(error)
            }
        }
    }
    fn graph_changed(&self) -> Result<(), Exception> {
        self.persist()
    }
}
