//! Native preapproval state and original SDK presentation callbacks (AOSP Apache-2.0).
use aim_binder_host::{
    local::Strong,
    parcel::{
        BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel, Reader, Result as ParcelResult,
    },
};
use aim_service_aidl::{
    ReadParcelable, WriteParcelable, dev_aim_server_iinstallerexternalbridge as aidl,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IntentSender {
    pub target: Binder,
}
impl ReadParcelable for IntentSender {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self {
            target: r.read_binder()?.ok_or(BAD_VALUE)?,
        })
    }
}
impl WriteParcelable for IntentSender {
    fn write_to(&self, p: &mut Parcel) {
        p.write_binder(Some(self.target));
    }
}
#[derive(Clone, Debug)]
pub struct Status {
    pub receiver: IntentSender,
    pub session_id: i32,
    pub package: Option<String>,
    pub legacy_status: i32,
    pub message: Option<String>,
    pub preapproval: bool,
    pub pending_installer: Option<String>,
}
pub struct BridgeOwner {
    owner: Strong,
    closed: std::sync::atomic::AtomicBool,
}
pub struct Icon {
    pub parcel: Option<super::codec::Object>,
    pub png: Option<Vec<u8>>,
}
fn byte_array(r: &mut Reader<'_>) -> ParcelResult<Option<Vec<u8>>> {
    let length = r.read_i32()?;
    if length < 0 {
        return Ok(None);
    }
    let start = r.position();
    let length = length as usize;
    r.skip(length.checked_add(3).ok_or(BAD_VALUE)? & !3)?;
    Ok(Some(r.since(start).0[..length].to_vec()))
}
fn transport(code: i32) -> Exception {
    Exception::new(
        EX_ILLEGAL_STATE,
        format!("installer external owner transport: {code}"),
    )
}
impl BridgeOwner {
    pub fn new(owner: Strong) -> Arc<Self> {
        Arc::new(Self {
            owner,
            closed: std::sync::atomic::AtomicBool::new(false),
        })
    }
    pub fn revoke(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
    }
    fn request(&self) -> Parcel {
        let mut p = Parcel::new();
        p.write_interface_token(aidl::DESCRIPTOR);
        p
    }
    fn read<T>(
        &self,
        code: u32,
        data: &Parcel,
        decode: impl FnOnce(&mut Reader<'_>) -> ParcelResult<T>,
    ) -> Result<T, Exception> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "installer external owner detached",
            ));
        }
        let reply = self.owner.transact(code, data, false).map_err(transport)?;
        let mut reader = reply.reader();
        reader.read_exception().map_err(transport)??;
        let value = decode(&mut reader).map_err(transport)?;
        if reader.remaining() != 0 {
            return Err(transport(BAD_VALUE));
        }
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "installer external owner detached during call",
            ));
        }
        Ok(value)
    }
    /// Stop outside native bootstrap locks before joining callback workers.
    pub fn close(&self) -> Result<(), Exception> {
        let result = self.read(aidl::CLOSE, &self.request(), |_| Ok(()));
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        result
    }
    pub fn native_install_environment(
        &self,
    ) -> Result<super::environment_producers::GuestSettings, Exception> {
        let bytes = self
            .read(
                aidl::GET_NATIVE_INSTALL_ENVIRONMENT,
                &self.request(),
                |reader| aim_service_aidl::read_byte_array(reader),
            )?
            .ok_or_else(|| transport(BAD_VALUE))?;
        let mut reader = Reader::new(&bytes, &[]);
        let page_size = u64::try_from(reader.read_i64().map_err(transport)?)
            .map_err(|_| transport(BAD_VALUE))?;
        let compat_16kb_disabled = reader.read_bool().map_err(transport)?;
        let headless_system_user = reader.read_bool().map_err(transport)?;
        let timezone = reader
            .read_string16()
            .map_err(transport)?
            .ok_or_else(|| transport(BAD_VALUE))?;
        if reader.remaining() != 0 || timezone.is_empty() {
            return Err(transport(BAD_VALUE));
        }
        Ok(super::environment_producers::GuestSettings {
            library: super::environment_producers::LibrarySettings {
                page_size,
                compat_16kb_disabled,
            },
            headless_system_user,
            timezone,
        })
    }
    pub fn zip_local_utc_offset(
        &self,
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> Result<i32, Exception> {
        let mut data = self.request();
        for value in [
            year,
            month as i32,
            day as i32,
            hour as i32,
            minute as i32,
            second as i32,
        ] {
            data.write_i32(value);
        }
        self.read(aidl::GET_ZIP_LOCAL_UTC_OFFSET, &data, |reader| {
            reader.read_i32()
        })
    }
    pub fn immutable(&self, sender: IntentSender) -> Result<bool, Exception> {
        let mut data = self.request();
        data.write_i32(1);
        sender.write_to(&mut data);
        self.read(aidl::IS_INTENT_SENDER_IMMUTABLE, &data, |r| r.read_bool())
    }
    pub fn removal_bridge(&self) -> Result<Binder, Exception> {
        self.read(aidl::GET_REMOVAL_BRIDGE, &self.request(), |r| {
            r.read_binder()?.ok_or(BAD_VALUE)
        })
    }
    pub fn available(&self) -> Result<bool, Exception> {
        self.read(
            aidl::IS_PREAPPROVAL_REQUEST_AVAILABLE,
            &self.request(),
            |r| r.read_bool(),
        )
    }
    pub fn commit_mutable_receiver_enforced(&self, uid: i32) -> Result<bool, Exception> {
        let mut data = self.request();
        data.write_i32(uid);
        self.read(aidl::IS_COMMIT_MUTABLE_RECEIVER_ENFORCED, &data, |r| {
            r.read_bool()
        })
    }
    pub fn secure_frp_active(&self) -> Result<bool, Exception> {
        self.read(aidl::IS_SECURE_FRP_ACTIVE, &self.request(), |r| {
            r.read_bool()
        })
    }
    pub fn user_action_policy(
        &self,
        installer: Option<&str>,
        uid: i32,
        user: i32,
    ) -> Result<u8, Exception> {
        let mut data = self.request();
        data.write_string16(installer);
        data.write_i32(uid);
        data.write_i32(user);
        self.read(aidl::GET_USER_ACTION_POLICY, &data, |r| {
            Ok(r.read_i32()? as u8)
        })
    }
    pub fn silent_install_target_allowed(
        &self,
        package: Option<&str>,
        sdk: i32,
    ) -> Result<bool, Exception> {
        let mut data = self.request();
        data.write_string16(package);
        data.write_i32(sdk);
        self.read(aidl::IS_SILENT_INSTALL_TARGET_ALLOWED, &data, |r| {
            r.read_bool()
        })
    }
    pub fn metadata_limit(&self) -> Result<i64, Exception> {
        self.read(aidl::GET_APP_METADATA_SIZE_LIMIT, &self.request(), |r| {
            r.read_i64()
        })
    }
    /// Body is the original updateSessionAppIcon's typed Bitmap, without its
    /// already-decoded session id. Original SDK accepts bitmap FD payloads.
    pub fn normalize_icon(&self, body: &super::codec::Object) -> Result<Icon, Exception> {
        let mut data = self.request();
        data.write_raw(&body.bytes, &body.objects);
        let encoded = self
            .read(aidl::NORMALIZE_INSTALLER_ICON, &data, byte_array)?
            .ok_or_else(|| transport(BAD_VALUE))?;
        let mut reader = Reader::new(&encoded, &[]);
        let parcel =
            byte_array(&mut reader)
                .map_err(transport)?
                .map(|bytes| super::codec::Object {
                    bytes,
                    objects: vec![],
                });
        let png = byte_array(&mut reader).map_err(transport)?;
        if reader.remaining() != 0 {
            return Err(transport(BAD_VALUE));
        }
        Ok(Icon { parcel, png })
    }
    pub fn app_state(
        &self,
        names: &[String],
        flags: super::constraints::Constraints,
        idle: bool,
    ) -> Result<u8, Exception> {
        let mut data = self.request();
        data.write_i32(names.len() as i32);
        for name in names {
            data.write_string16(Some(name));
        }
        data.write_i32(i32::from(flags.0));
        data.write_bool(idle);
        self.read(aidl::GET_CONSTRAINT_APP_STATE, &data, |r| {
            Ok(r.read_i32()? as u8)
        })
    }
    pub fn dependencies(&self, names: &[String]) -> Result<Vec<String>, Exception> {
        let mut data = self.request();
        data.write_i32(names.len() as i32);
        for name in names {
            data.write_string16(Some(name));
        }
        self.read(aidl::GET_CONSTRAINT_DEPENDENCY_PACKAGES, &data, |reader| {
            aim_service_aidl::read_string_list(reader)?
                .ok_or(BAD_VALUE)?
                .into_iter()
                .map(|name| name.ok_or(BAD_VALUE))
                .collect()
        })
    }
    pub fn watch_app_state(&self, callback: Binder) -> Result<(), Exception> {
        let mut data = self.request();
        data.write_binder(Some(callback));
        self.post(aidl::WATCH_APP_STATE, &data)
    }
    pub fn request_idle_job(&self, callback: Binder) -> Result<(), Exception> {
        let mut data = self.request();
        data.write_binder(Some(callback));
        self.post(aidl::REQUEST_IDLE_JOB, &data)
    }
    fn post(&self, code: u32, data: &Parcel) -> Result<(), Exception> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "installer external owner detached",
            ));
        }
        self.owner.transact(code, data, true).map_err(transport)?;
        Ok(())
    }
    /// Raw body is the original requestUserPreapproval's two typed objects,
    /// including object offsets. The original SDK decodes styles and FD icons;
    /// this never calls original PMS/PackageInstallerSession.
    pub fn prepare(&self, session: i32, body: &super::codec::Object) -> Result<String, Exception> {
        let mut data = self.request();
        data.write_i32(session);
        data.write_raw(&body.bytes, &body.objects);
        self.read(aidl::PREPARE_PREAPPROVAL, &data, |r| {
            r.read_string16()?.ok_or(BAD_VALUE)
        })
    }
    pub fn clear(&self, session: i32) -> Result<(), Exception> {
        let mut data = self.request();
        data.write_i32(session);
        self.read(aidl::CLEAR_PREAPPROVAL, &data, |_| Ok(()))
    }
    pub fn preapproval_status(
        &self,
        session: i32,
        legacy: i32,
        message: Option<&str>,
        installer: Option<&str>,
        preapproval_extra: bool,
    ) -> Result<(), Exception> {
        let mut data = self.request();
        data.write_i32(session);
        data.write_i32(legacy);
        data.write_string16(message);
        data.write_string16(installer);
        data.write_bool(preapproval_extra);
        self.read(aidl::SEND_PREAPPROVAL_STATUS, &data, |_| Ok(()))
    }
    pub fn deliver(&self, status: &Status) -> Result<(), Exception> {
        let mut data = self.request();
        data.write_i32(1);
        status.receiver.write_to(&mut data);
        data.write_i32(status.session_id);
        data.write_string16(status.package.as_deref());
        data.write_i32(status.legacy_status);
        data.write_string16(status.message.as_deref());
        data.write_bool(status.preapproval);
        data.write_string16(status.pending_installer.as_deref());
        self.read(aidl::SEND_SESSION_STATUS, &data, |_| Ok(()))
    }
    pub fn deliver_with_warnings(&self,status:&Status,warnings:&[String])->Result<(),Exception> {
        let mut data=self.request();data.write_i32(1);status.receiver.write_to(&mut data);
        data.write_i32(status.session_id);data.write_string16(status.package.as_deref());
        data.write_i32(status.legacy_status);data.write_string16(status.message.as_deref());
        data.write_bool(status.preapproval);data.write_string16(status.pending_installer.as_deref());
        let values=warnings.iter().cloned().map(Some).collect::<Vec<_>>();
        aim_service_aidl::write_string_list(&mut data,Some(&values));
        self.read(aidl::SEND_SESSION_STATUS_WITH_WARNINGS,&data,|_|Ok(()))
    }
    pub fn pending_streaming(
        &self,
        receiver: IntentSender,
        id: i32,
        message: &str,
    ) -> Result<(), Exception> {
        let mut data = self.request();
        data.write_i32(1);
        receiver.write_to(&mut data);
        data.write_i32(id);
        data.write_string16(Some(message));
        self.read(aidl::SEND_PENDING_STREAMING, &data, |_| Ok(()))
    }
    pub fn constraint_callback(&self, callback: Binder, satisfied: bool) -> Result<(), Exception> {
        let mut data = self.request();
        data.write_binder(Some(callback));
        data.write_bool(satisfied);
        self.read(aidl::SEND_CONSTRAINT_CALLBACK, &data, |_| Ok(()))
    }
    pub fn constraint_intent(
        &self,
        callback: IntentSender,
        names: &[String],
        flags: u8,
        satisfied: bool,
    ) -> Result<(), Exception> {
        let mut data = self.request();
        data.write_i32(1);
        callback.write_to(&mut data);
        data.write_i32(names.len() as i32);
        for name in names {
            data.write_string16(Some(name));
        }
        data.write_i32(i32::from(flags));
        data.write_bool(satisfied);
        self.read(aidl::SEND_CONSTRAINT_INTENT, &data, |_| Ok(()))
    }
}
#[derive(Clone, Debug)]
pub struct Session {
    pub id: i32,
    pub installer_uid: u32,
    pub prepared: bool,
    pub sealed: bool,
    pub destroyed: bool,
    pub multi_package: bool,
    pub committed: bool,
    pub parent: i32,
}
#[derive(Clone, Debug, Default)]
struct Approval {
    requested: bool,
    manually_accepted: bool,
}
pub enum Resume {
    Install(i32),
    Preapproval(i32),
    Rejected(i32),
}
pub type ResumeOwner = Arc<dyn Fn(Resume) -> Result<(), Exception> + Send + Sync>;
pub type UserAction = Arc<dyn Fn(i32, bool) -> Result<Option<String>, Exception> + Send + Sync>;
enum Message {
    Check(i32),
    Unavailable(i32, bool),
    Resume(Resume),
}
pub struct Owner {
    approvals: Mutex<BTreeMap<i32, Approval>>,
    bridge: Arc<BridgeOwner>,
    resume: ResumeOwner,
    user_action: UserAction,
    queue: mpsc::Sender<Option<Message>>,
    errors: Mutex<Vec<String>>,
    stopped: std::sync::atomic::AtomicBool,
}
pub struct Worker {
    owner: Arc<Owner>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}
impl Owner {
    pub fn start(
        bridge: Arc<BridgeOwner>,
        resume: ResumeOwner,
        user_action: UserAction,
    ) -> (Arc<Self>, Worker) {
        let (send, receive) = mpsc::channel();
        let owner = Arc::new(Self {
            approvals: Mutex::new(BTreeMap::new()),
            bridge,
            resume,
            user_action,
            queue: send,
            errors: Mutex::new(vec![]),
            stopped: std::sync::atomic::AtomicBool::new(false),
        });
        let running = owner.clone();
        let thread = thread::spawn(move || {
            while let Ok(Some(message)) = receive.recv() {
                if running.stopped.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                let result = match message {
                    Message::Check(id) => {
                        let accepted = running.manually_accepted(id);
                        (running.user_action)(id, accepted).and_then(|installer| {
                            running.bridge.preapproval_status(
                                id,
                                1,
                                None,
                                installer.as_deref(),
                                true,
                            )
                        })
                    }
                    Message::Unavailable(id, extra) => running.bridge.preapproval_status(
                        id,
                        -129,
                        Some("Request user pre-approval is currently not available."),
                        None,
                        extra,
                    ),
                    Message::Resume(resume) => (running.resume)(resume),
                };
                if let Err(error) = result {
                    running
                        .errors
                        .lock()
                        .unwrap()
                        .push(format!("preapproval callback failed: {error:?}"));
                }
            }
        });
        (
            owner.clone(),
            Worker {
                owner,
                thread: Some(thread),
            },
        )
    }
    pub fn request(
        &self,
        session: &Session,
        uid: u32,
        body: &super::codec::Object,
    ) -> Result<String, Exception> {
        if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "preapproval owner stopped",
            ));
        }
        if uid != 0 && uid != session.installer_uid {
            return Err(Exception::security("Session does not belong to caller"));
        }
        if session.multi_package {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!(
                    "Session {} is a parent of multi-package session and requestUserPreapproval on the parent session isn't supported.",
                    session.id
                ),
            ));
        }
        if !session.prepared {
            return Err(Exception::new(EX_ILLEGAL_STATE, "Session is not prepared"));
        }
        if session.sealed || session.destroyed {
            return Err(Exception::security("Session is destroyed or sealed"));
        }
        let package = self.bridge.prepare(session.id, body)?;
        if !self.bridge.available()? {
            self.queue
                .send(Some(Message::Unavailable(
                    session.id,
                    self.requested(session.id),
                )))
                .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "preapproval worker stopped"))?;
            return Ok(package);
        }
        let mut approvals = self.approvals.lock().unwrap();
        let approval = approvals.entry(session.id).or_default();
        if approval.requested {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Session already requested preapproval",
            ));
        }
        approval.requested = true;
        drop(approvals);
        self.queue
            .send(Some(Message::Check(session.id)))
            .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "preapproval worker stopped"))?;
        Ok(package)
    }
    /// Caller INSTALL_PACKAGES enforcement is performed before session lookup.
    /// Accepted flags belong to the child; committed parent resumes installation.
    pub fn permissions_result(&self, session: &Session, accepted: bool) -> Result<(), Exception> {
        if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "preapproval owner stopped",
            ));
        }
        let mut approvals = self.approvals.lock().unwrap();
        let approval = approvals.entry(session.id).or_default();
        if !session.sealed && !approval.requested {
            return Err(Exception::security("Must be sealed to accept permissions"));
        }
        let root = if session.parent != -1 && session.committed {
            session.parent
        } else {
            session.id
        };
        if accepted {
            approval.manually_accepted = true;
            drop(approvals);
            let message = if session.committed {
                Message::Resume(Resume::Install(root))
            } else {
                Message::Check(root)
            };
            self.queue
                .send(Some(message))
                .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "preapproval worker stopped"))
        } else {
            drop(approvals);
            self.queue
                .send(Some(Message::Resume(Resume::Rejected(root))))
                .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "preapproval worker stopped"))
        }
    }
    pub fn requested(&self, session: i32) -> bool {
        self.approvals
            .lock()
            .unwrap()
            .get(&session)
            .is_some_and(|v| v.requested)
    }
    pub fn manually_accepted(&self, session: i32) -> bool {
        self.approvals
            .lock()
            .unwrap()
            .get(&session)
            .is_some_and(|v| v.manually_accepted)
    }
    pub fn remove(&self, session: i32) -> Result<(), Exception> {
        self.approvals.lock().unwrap().remove(&session);
        self.bridge.clear(session)
    }
    pub fn stop(&self) {
        let _ = self.queue.send(None);
    }
    pub fn errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.owner
            .stopped
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = self.owner.queue.send(None);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
