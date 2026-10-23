//! Production pre-reboot staged verifier and checkpoint/apexd ordering.
use super::{
    Record, Session,
    native::{LitePolicySource, StagedExecutor},
    pipeline, storage,
};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, Reader};
use aim_service_aidl::dev_aim_server_iinstallerpreparationbridge as api;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub ready: bool,
    pub applied: bool,
    pub failed: bool,
    pub error_code: i32,
    pub error_message: Option<String>,
}
pub type Records = Arc<dyn Fn() -> Vec<(Session, Record)> + Send + Sync>;
pub struct Owner {
    bridge: crate::system::ServiceOwner,
    native: Arc<pipeline::Native>,
    lite: LitePolicySource,
    disk: Arc<Mutex<storage::Store>>,
    records: Records,
    statuses: Mutex<BTreeMap<i32, Status>>,
    operation: Mutex<()>,
    stopped: std::sync::atomic::AtomicBool,
}
fn transport(code: i32) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, format!("staged transport: {code}"))
}
impl Owner {
    pub fn new(
        bridge: crate::system::ServiceOwner,
        native: Arc<pipeline::Native>,
        lite: LitePolicySource,
        disk: Arc<Mutex<storage::Store>>,
        records: Records,
    ) -> Result<Arc<Self>, Exception> {
        let statuses = disk
            .lock()
            .unwrap()
            .staged_states()
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.message))?;
        Ok(Arc::new(Self {
            bridge,
            native,
            lite,
            disk,
            records,
            statuses: Mutex::new(statuses),
            operation: Mutex::new(()),
            stopped: std::sync::atomic::AtomicBool::new(false),
        }))
    }
    fn call<T>(
        &self,
        code: u32,
        fill: impl FnOnce(&mut Parcel),
        decode: impl FnOnce(&mut Reader<'_>) -> Result<T, i32>,
    ) -> Result<T, Exception> {
        if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(EX_ILLEGAL_STATE, "staged owner stopped"));
        }
        let mut data = Parcel::new();
        data.write_interface_token(api::DESCRIPTOR);
        fill(&mut data);
        let reply = self
            .bridge
            .transact(code, &data, false)
            .map_err(transport)?;
        let mut reader = reply.reader();
        reader.read_exception().map_err(transport)??;
        let value = decode(&mut reader).map_err(transport)?;
        if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "staged owner stopped during effect",
            ));
        }
        if reader.remaining() != 0 {
            return Err(transport(BAD_VALUE));
        }
        Ok(value)
    }
    pub fn stop(&self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn close(&self) {
        self.stop();
    }
    pub fn executor(self: &Arc<Self>) -> StagedExecutor {
        let owner = self.clone();
        Arc::new(move |id, members| owner.verify(id, members))
    }
    fn save(&self, id: i32, status: Status) -> Result<(), Exception> {
        let mut states = self.statuses.lock().unwrap();
        states.insert(id, status);
        self.disk
            .lock()
            .unwrap()
            .write_staged(&(self.records)(), &states)
            .map_err(|error| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!(
                        "staged state committed={}: {}",
                        error.committed, error.message
                    ),
                )
            })
    }
    pub fn status(&self, id: i32) -> Option<Status> {
        self.statuses.lock().unwrap().get(&id).cloned()
    }
    /// Pre-reboot verification. A package the install would reject fails
    /// with its install status, as an ordinary commit's does.
    pub fn verify(
        &self,
        id: i32,
        members: Vec<(Session, Record, String)>,
    ) -> Result<crate::package::staging::ReadySession, pipeline::Error> {
        let _operation = self.operation.lock().unwrap();
        self.verify_locked(id, members)
    }
    fn verify_apks(&self, apk: Vec<(Session, Record, String)>) -> Result<(), pipeline::Error> {
        let install = pipeline::Error::Install;
        let lite = (self.lite)().map_err(|error| pipeline::Error::Owner {
            committed: false,
            error,
        })?;
        let code = pipeline::verify_batch(&self.native.apks, apk, &lite).map_err(install)?;
        let base = self.native.snapshots.capture();
        let reservation = self
            .native
            .environment
            .reserve(code, &base)
            .map_err(install)?;
        let requests = reservation.requests().map_err(install)?;
        // Validate against the cloned native scan; pre-reboot verification
        // does not create app data or publish installed package state.
        base.owner()
            .prepare_live_installs(
                requests,
                reservation.users(),
                reservation.build_debuggable(),
                &self.native.apks.files,
            )
            .map_err(|error| {
                install(pipeline::Failure {
                    legacy_status: error.status,
                    committed: false,
                    message: error.message,
                })
            })?;
        Ok(())
    }
    fn verify_locked(
        &self,
        id: i32,
        members: Vec<(Session, Record, String)>,
    ) -> Result<crate::package::staging::ReadySession, pipeline::Error> {
        let owner = |error| pipeline::Error::Owner {
            committed: false,
            error,
        };
        if members.is_empty()
            || members.iter().any(|(session, _, _)| {
                !session.prepared
                    || !session.sealed
                    || session.destroyed
                    || !session.parameters.staged
            })
        {
            return Err(owner(Exception::illegal_argument(
                "invalid immutable staged graph",
            )));
        }
        let checkpoint = self
            .call(
                api::SUPPORTS_CHECKPOINT,
                |_| {},
                |reader| reader.read_bool(),
            )
            .map_err(owner)?;
        // PackageSessionVerifier.checkActiveSessions: committed staged roots
        // that are neither destroyed nor terminal, this one included.
        let statuses = self.statuses.lock().unwrap().clone();
        let active = (self.records)()
            .iter()
            .filter(|(session, _)| {
                session.parameters.staged
                    && session.parent == -1
                    && session.committed
                    && !session.destroyed
                    && !statuses
                        .get(&session.id)
                        .is_some_and(|status| status.applied || status.failed)
            })
            .count();
        if !checkpoint && active > 1 {
            // PackageManager.INSTALL_FAILED_OTHER_STAGED_SESSION_IN_PROGRESS
            return Err(pipeline::Error::Install(pipeline::Failure {
                legacy_status: -119,
                committed: false,
                message: "Cannot stage multiple sessions without checkpoint support".into(),
            }));
        }
        let apex_children = members
            .iter()
            .filter(|(session, _, _)| session.parameters.install_flags & 0x20000 != 0)
            .map(|(session, _, _)| session.id)
            .collect::<Vec<_>>();
        let apk = members
            .iter()
            .filter(|(session, _, _)| session.parameters.install_flags & 0x20000 == 0)
            .cloned()
            .collect::<Vec<_>>();
        if !apk.is_empty() {
            self.verify_apks(apk)?;
        }
        let root_record = (self.records)()
            .into_iter()
            .find(|(session, _)| session.id == id)
            .ok_or_else(|| owner(Exception::illegal_argument("staged root absent")))?
            .1;
        let rollback_id = self
            .call(
                api::GET_ROLLBACK_ID,
                |p| {
                    p.write_i32(id);
                    p.write_bool(root_record.params.install_flags & 0x40000 != 0);
                    p.write_bool(root_record.params.install_reason == 5);
                },
                |r| r.read_i32(),
            )
            .map_err(owner)?;
        if !apex_children.is_empty() {
            let apex_ids = if members.len() == 1 && members[0].0.id == id {
                Vec::new()
            } else {
                apex_children.clone()
            };
            self.call(
                api::SUBMIT_APEX,
                |p| {
                    p.write_i32(id);
                    aim_service_aidl::write_int_array(p, Some(&apex_ids));
                    p.write_bool(root_record.params.install_reason == 5);
                    p.write_i32(rollback_id);
                },
                |_| Ok(()),
            )
            .map_err(owner)?;
        }
        if checkpoint {
            self.call(api::START_CHECKPOINT, |_| {}, |_| Ok(()))
                .map_err(owner)?;
        }
        self.save(
            id,
            Status {
                ready: true,
                ..Default::default()
            },
        )
        .map_err(owner)?;
        if !apex_children.is_empty() {
            if let Err(error) = self.call(api::MARK_APEX_READY, |p| p.write_i32(id), |_| Ok(())) {
                self.save(
                    id,
                    Status {
                        failed: true,
                        error_code: -110,
                        error_message: Some(error.message.clone()),
                        ..Default::default()
                    },
                )
                .map_err(owner)?;
                return Err(owner(error));
            }
        }
        Ok(crate::package::staging::ReadySession {
            id,
            apex_children: if members.len() == 1 && members[0].0.id == id {
                Vec::new()
            } else {
                apex_children
            },
        })
    }
    pub fn abort(&self, id: i32) -> Result<bool, Exception> {
        self.call(api::ABORT_APEX, |p| p.write_i32(id), |r| r.read_bool())
    }
    /// `StagedSession.setSessionFailed`.
    pub fn failed(&self, id: i32, code: i32, message: String) -> Result<(), Exception> {
        self.save(
            id,
            Status {
                failed: true,
                error_code: code,
                error_message: Some(message),
                ..Default::default()
            },
        )
    }
    pub fn applied(&self, id: i32, apex: bool) -> Result<(), Exception> {
        if apex {
            self.call(api::MARK_APEX_SUCCESSFUL, |p| p.write_i32(id), |_| Ok(()))?;
        }
        self.save(
            id,
            Status {
                applied: true,
                ..Default::default()
            },
        )
    }
}
