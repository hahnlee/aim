//! Native IPackageInstaller transport. Policy and stage operations are required
//! owners, separate from the session state machine and generated wire codecs.
use super::{
    Event, Record, Session, SessionNode, Sessions,
    codec::{SessionInfo, SessionParams},
};
use aim_binder_host::{
    local::{Call, Reply, Service},
    parcel::{
        BAD_VALUE, Binder, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel,
        UNKNOWN_TRANSACTION,
    },
};
use aim_service_aidl::android_content_pm_ipackageinstaller as aidl;
use std::sync::Arc;

/// Owner callbacks perform real user/permission/app-ops normalization, install
/// storage preparation, persistent scheduling and callback capability lifetime.
/// Implementations must never forward these operations to original PMS.
pub enum ServiceSetting {
    Unlimited(Option<String>),
    Throttle(i64),
    Staged(bool),
    Apex(bool),
    Verification(i32),
}
pub enum ArchiveAction {
    Archive {
        package: String,
        caller: String,
        flags: i32,
        receiver: super::preapproval::IntentSender,
        user: i32,
    },
    Unarchive {
        package: String,
        caller: String,
        receiver: super::preapproval::IntentSender,
        user: i32,
        show_confirmation: bool,
    },
    Install {
        package: super::archiver::ArchivedPackage,
        params: SessionParams,
        receiver: super::preapproval::IntentSender,
        installer: String,
        user: i32,
    },
    Report {
        id: i32,
        status: i32,
        required: i64,
        action: Option<super::preapproval::IntentSender>,
        user: i32,
    },
}
pub enum ConstraintReceiver {
    Callback(Binder),
    Intent(super::preapproval::IntentSender),
}
pub struct RemoteCallback(pub Option<Binder>);
impl aim_service_aidl::ReadParcelable for RemoteCallback {
    fn read_from(
        reader: &mut aim_binder_host::parcel::Reader<'_>,
    ) -> aim_binder_host::parcel::Result<Self> {
        Ok(Self(reader.read_binder()?))
    }
}
pub trait Owners: Send + Sync {
    fn uninstall(
        &self,
        request: super::removal::Request,
        receiver: Option<super::preapproval::IntentSender>,
    ) -> Result<(), Exception>;
    fn archive(&self, uid: u32, pid: i32, action: ArchiveAction) -> Result<(), Exception>;
    fn update_icon(&self, uid: u32, id: i32, body: super::codec::Object) -> Result<(), Exception>;
    fn install_constraints(
        &self,
        uid: u32,
        installer: Option<String>,
        names: Option<Vec<Option<String>>>,
        constraints: Option<super::constraints::Constraints>,
        receiver: ConstraintReceiver,
        timeout: i64,
    ) -> Result<(), Exception>;
    fn install_existing(
        &self,
        uid: u32,
        pid: i32,
        request: super::existing::Request,
    ) -> Result<(), Exception>;
    fn permissions_result(&self, uid: u32, id: i32, accepted: bool) -> Result<(), Exception>;
    fn normalize(
        &self,
        uid: u32,
        pid: i32,
        params: SessionParams,
        installer: Option<String>,
        tag: Option<String>,
        user: i32,
    ) -> Result<(Record, bool), Exception>;
    fn staged_status(&self,_id:i32)->Result<Option<super::staged_owner::Status>,Exception>{Err(Exception::new(EX_UNSUPPORTED_OPERATION,"staged owner unavailable"))}
    fn session_created(&self, id: i32, user: u32) -> Result<(), Exception>;
    fn prepare_stage(&self, session: &Session, record: &Record) -> Result<(), Exception>;
    fn session_operations(&self) -> Option<Arc<dyn super::SessionOperations>>;
    fn publish_session(&self, node: Arc<SessionNode>) -> Result<Binder, Exception>;
    fn notify(&self, event: Event);
    fn enforce_cross_user(&self, uid: u32, user: i32, operation: &str) -> Result<(), Exception>;
    fn check_package(&self, uid: u32, package: Option<&str>) -> Result<(), Exception>;
    fn can_query(&self, uid: u32, package: Option<&str>) -> Result<bool, Exception>;
    fn can_read_paths(&self, uid: u32) -> Result<bool, Exception>;
    fn resolved_path(&self, id: i32) -> Result<Option<String>, Exception>;
    fn is_verifier(&self, uid: u32) -> Result<bool, Exception>;
    fn update_label(&self, uid: u32, id: i32, label: Option<String>) -> Result<(), Exception>;
    fn register_callback(
        &self,
        uid: u32,
        callback: Option<Binder>,
        user: i32,
    ) -> Result<(), Exception>;
    fn unregister_callback(&self, callback: Option<Binder>) -> Result<(), Exception>;
    fn set_service_policy(&self, uid: u32, setting: ServiceSetting) -> Result<(), Exception>;
    fn abandon_stage(&self, ids: &[i32]) -> Result<(), Exception>;
}
pub struct Endpoint {
    pub sessions: Arc<Sessions>,
    pub owners: Arc<dyn Owners>,
}
impl Endpoint {
    fn info(
        &self,
        session: &Session,
        record: &Record,
        icon: bool,
        uid: u32,
    ) -> Result<Option<SessionInfo>, Exception> {
        if session.parameters.staged && session.destroyed {
            return Ok(None);
        }
        let mut info = record.info(session, icon, uid);
        if session.parameters.staged {
            if let Some(status)=self.owners.staged_status(session.id)?{
                info.session_ready=status.ready;info.session_applied=status.applied;info.session_failed=status.failed;
                info.session_error_code=status.error_code;info.session_error_message=status.error_message;
            }
        }
        if uid != session.installer_uid
            && !self
                .owners
                .can_query(uid, info.app_package_name.as_deref())?
        {
            return Ok(None);
        }
        if self.owners.can_read_paths(uid)? {
            info.resolved_base_code_path = self.owners.resolved_path(session.id)?;
        }
        Ok(Some(info))
    }
}
impl Service for Endpoint {
    fn descriptor(&self) -> &str {
        aidl::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        enum Action {
            Archive(ArchiveAction),
            Uninstall(
                super::removal::VersionedPackage,
                Option<String>,
                i32,
                Option<super::preapproval::IntentSender>,
                i32,
                bool,
            ),
            Icon(i32, super::codec::Object),
            Constraints(
                Option<String>,
                Option<Vec<Option<String>>>,
                Option<super::constraints::Constraints>,
                ConstraintReceiver,
                i64,
            ),
            Existing(super::existing::Request),
            Permissions(i32, bool),
            Create(aidl::CreateSession<SessionParams>),
            Label(i32, Option<String>),
            Register(Option<Binder>, i32),
            Unregister(Option<Binder>),
            Policy(ServiceSetting),
            Open(i32),
            Info(i32),
            All(i32),
            Mine(Option<String>, i32),
            Staged,
            Abandon(i32),
        }
        let action = match call.code {
            aidl::UNINSTALL => {
                let args = aidl::Uninstall::<
                    super::removal::VersionedPackage,
                    super::preapproval::IntentSender,
                >::read(&mut call.data)?;
                Action::Uninstall(
                    args.versioned_package.ok_or(BAD_VALUE)?,
                    args.caller_package_name,
                    args.flags,
                    args.status_receiver,
                    args.user_id,
                    false,
                )
            }
            aidl::UNINSTALL_EXISTING_PACKAGE => {
                let args = aidl::UninstallExistingPackage::<
                    super::removal::VersionedPackage,
                    super::preapproval::IntentSender,
                >::read(&mut call.data)?;
                Action::Uninstall(
                    args.versioned_package.ok_or(BAD_VALUE)?,
                    args.caller_package_name,
                    0,
                    args.status_receiver,
                    args.user_id,
                    true,
                )
            }
            aidl::REQUEST_ARCHIVE => {
                let args = aidl::RequestArchive::<
                    super::preapproval::IntentSender,
                    super::removal::UserHandle,
                >::read(&mut call.data)?;
                Action::Archive(ArchiveAction::Archive {
                    package: args.package_name.ok_or(BAD_VALUE)?,
                    caller: args.caller_package_name.ok_or(BAD_VALUE)?,
                    flags: args.flags,
                    receiver: args.status_receiver.ok_or(BAD_VALUE)?,
                    user: args.user_handle.ok_or(BAD_VALUE)?.0,
                })
            }
            aidl::REQUEST_UNARCHIVE => {
                let args = aidl::RequestUnarchive::<
                    super::preapproval::IntentSender,
                    super::removal::UserHandle,
                >::read(&mut call.data)?;
                Action::Archive(ArchiveAction::Unarchive {
                    package: args.package_name.ok_or(BAD_VALUE)?,
                    caller: args.caller_package_name.ok_or(BAD_VALUE)?,
                    receiver: args.status_receiver.ok_or(BAD_VALUE)?,
                    user: args.user_handle.ok_or(BAD_VALUE)?.0,
                    show_confirmation: false,
                })
            }
            aidl::INSTALL_PACKAGE_ARCHIVED => {
                let args = aidl::InstallPackageArchived::<
                    super::archiver::ArchivedPackage,
                    SessionParams,
                    super::preapproval::IntentSender,
                    super::removal::UserHandle,
                >::read(&mut call.data)?;
                Action::Archive(ArchiveAction::Install {
                    package: args.archived_package_parcel.ok_or(BAD_VALUE)?,
                    params: args.params.ok_or(BAD_VALUE)?,
                    receiver: args.status_receiver.ok_or(BAD_VALUE)?,
                    installer: args.installer_package_name.ok_or(BAD_VALUE)?,
                    user: args.user_handle.ok_or(BAD_VALUE)?.0,
                })
            }
            aidl::REPORT_UNARCHIVAL_STATUS => {
                let args = aidl::ReportUnarchivalStatus::<
                    super::preapproval::IntentSender,
                    super::removal::UserHandle,
                >::read(&mut call.data)?;
                Action::Archive(ArchiveAction::Report {
                    id: args.unarchive_id,
                    status: args.status,
                    required: args.required_storage_bytes,
                    action: args.user_action_intent,
                    user: args.user_handle.ok_or(BAD_VALUE)?.0,
                })
            }
            aidl::UPDATE_SESSION_APP_ICON => {
                call.data.enforce_interface(aidl::DESCRIPTOR)?;
                let id = call.data.read_i32()?;
                let start = call.data.position();
                call.data.skip(call.data.remaining())?;
                let (bytes, objects) = call.data.since(start);
                Action::Icon(
                    id,
                    super::codec::Object {
                        bytes: bytes.to_vec(),
                        objects,
                    },
                )
            }
            aidl::CHECK_INSTALL_CONSTRAINTS => {
                let args = aidl::CheckInstallConstraints::<
                    super::constraints::Constraints,
                    RemoteCallback,
                >::read(&mut call.data)?;
                let callback = args
                    .callback
                    .and_then(|callback| callback.0)
                    .ok_or(BAD_VALUE)?;
                Action::Constraints(
                    args.installer_package_name,
                    args.package_names,
                    args.constraints,
                    ConstraintReceiver::Callback(callback),
                    0,
                )
            }
            aidl::WAIT_FOR_INSTALL_CONSTRAINTS => {
                let args = aidl::WaitForInstallConstraints::<
                    super::constraints::Constraints,
                    super::preapproval::IntentSender,
                >::read(&mut call.data)?;
                Action::Constraints(
                    args.installer_package_name,
                    args.package_names,
                    args.constraints,
                    ConstraintReceiver::Intent(args.callback.ok_or(BAD_VALUE)?),
                    args.timeout,
                )
            }
            aidl::INSTALL_EXISTING_PACKAGE => {
                let args = aidl::InstallExistingPackage::<super::existing::IntentSender>::read(
                    &mut call.data,
                )?;
                Action::Existing(super::existing::Request {
                    package: args.package_name,
                    user: args.user_id,
                    flags: args.install_flags,
                    reason: args.install_reason,
                    allowlisted_permissions: args.white_listed_permissions,
                    receiver: args.status_receiver,
                })
            }
            aidl::SET_PERMISSIONS_RESULT => {
                let args = aidl::SetPermissionsResult::read(&mut call.data)?;
                Action::Permissions(args.session_id, args.accepted)
            }
            aidl::UPDATE_SESSION_APP_LABEL => {
                let args = aidl::UpdateSessionAppLabel::read(&mut call.data)?;
                Action::Label(args.session_id, args.app_label)
            }
            aidl::SET_ALLOW_UNLIMITED_SILENT_UPDATES => Action::Policy(ServiceSetting::Unlimited(
                aidl::SetAllowUnlimitedSilentUpdates::read(&mut call.data)?.installer_package_name,
            )),
            aidl::SET_SILENT_UPDATES_THROTTLE_TIME => Action::Policy(ServiceSetting::Throttle(
                aidl::SetSilentUpdatesThrottleTime::read(&mut call.data)?.throttle_time_in_seconds,
            )),
            aidl::REGISTER_CALLBACK => {
                let args = aidl::RegisterCallback::read(&mut call.data)?;
                Action::Register(args.callback, args.user_id)
            }
            aidl::UNREGISTER_CALLBACK => {
                Action::Unregister(aidl::UnregisterCallback::read(&mut call.data)?.callback)
            }
            aidl::BYPASS_NEXT_STAGED_INSTALLER_CHECK => Action::Policy(ServiceSetting::Staged(
                aidl::BypassNextStagedInstallerCheck::read(&mut call.data)?.value,
            )),
            aidl::BYPASS_NEXT_ALLOWED_APEX_UPDATE_CHECK => Action::Policy(ServiceSetting::Apex(
                aidl::BypassNextAllowedApexUpdateCheck::read(&mut call.data)?.value,
            )),
            aidl::DISABLE_VERIFICATION_FOR_UID => Action::Policy(ServiceSetting::Verification(
                aidl::DisableVerificationForUid::read(&mut call.data)?.uid,
            )),
            aidl::CREATE_SESSION => Action::Create(aidl::CreateSession::read(&mut call.data)?),
            aidl::OPEN_SESSION => Action::Open(aidl::OpenSession::read(&mut call.data)?.session_id),
            aidl::GET_SESSION_INFO => {
                Action::Info(aidl::GetSessionInfo::read(&mut call.data)?.session_id)
            }
            aidl::GET_ALL_SESSIONS => {
                Action::All(aidl::GetAllSessions::read(&mut call.data)?.user_id)
            }
            aidl::GET_MY_SESSIONS => {
                let a = aidl::GetMySessions::read(&mut call.data)?;
                Action::Mine(a.installer_package_name, a.user_id)
            }
            aidl::GET_STAGED_SESSIONS => {
                aidl::GetStagedSessions::read(&mut call.data)?;
                Action::Staged
            }
            aidl::ABANDON_SESSION => {
                Action::Abandon(aidl::AbandonSession::read(&mut call.data)?.session_id)
            }
            _ => {
                if !aidl::METHODS.iter().any(|(code, _)| *code == call.code) {
                    return Err(UNKNOWN_TRANSACTION);
                }
                call.data.enforce_interface(aidl::DESCRIPTOR)?;
                let mut reply = Parcel::new();
                reply.write_exception(&Exception::new(
                    EX_UNSUPPORTED_OPERATION,
                    "Native installer dependency is unavailable",
                ));
                return Ok(reply);
            }
        };
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        let uid = call.sender_euid;
        let mut reply = Parcel::new();
        let result = (|| -> Result<(), Exception> {
            match action {
                Action::Archive(action) => {
                    self.owners.archive(uid, call.sender_pid as i32, action)?;
                    reply.write_no_exception();
                }
                Action::Uninstall(package, caller, flags, receiver, user, existing_only) => {
                    self.owners.uninstall(
                        super::removal::Request {
                            package: package.name,
                            version: package.version,
                            caller_package: caller,
                            uid,
                            pid: call.sender_pid as i32,
                            user,
                            flags,
                            existing_only,
                        },
                        receiver,
                    )?;
                    reply.write_no_exception();
                }
                Action::Icon(id, body) => {
                    self.owners.update_icon(uid, id, body)?;
                    reply.write_no_exception();
                }
                Action::Constraints(installer, names, constraints, receiver, timeout) => {
                    self.owners.install_constraints(
                        uid,
                        installer,
                        names,
                        constraints,
                        receiver,
                        timeout,
                    )?;
                    reply.write_no_exception();
                }
                Action::Existing(request) => {
                    self.owners
                        .install_existing(uid, call.sender_pid as i32, request)?;
                    reply.write_no_exception();
                }
                Action::Permissions(id, accepted) => {
                    self.owners.permissions_result(uid, id, accepted)?;
                    reply.write_no_exception();
                }
                Action::Label(id, label) => {
                    self.owners.update_label(uid, id, label)?;
                    aidl::write_update_session_app_label_reply(&mut reply);
                }
                Action::Register(callback, user) => {
                    self.owners.register_callback(uid, callback, user)?;
                    aidl::write_register_callback_reply(&mut reply);
                }
                Action::Unregister(callback) => {
                    self.owners.unregister_callback(callback)?;
                    aidl::write_unregister_callback_reply(&mut reply);
                }
                Action::Policy(setting) => {
                    self.owners.set_service_policy(uid, setting)?;
                    reply.write_no_exception();
                }
                Action::Create(a) => {
                    let params = a
                        .params
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "null SessionParams"))?;
                    // No capability in SessionParams has been retained by this
                    // endpoint. Refuse every such input before normalization can
                    // invoke policy or allocate a session, including Bitmap FDs.
                    if params.has_capabilities() {
                        return Err(Exception::new(EX_UNSUPPORTED_OPERATION,
                            "Native install parcel file capability owner is unavailable"));
                    }
                    let (record, permission) = self.owners.normalize(
                        uid,
                        call.sender_pid as i32,
                        params,
                        a.installer_package_name,
                        a.installer_attribution_tag,
                        a.user_id,
                    )?;
                    let user = record.user;
                    let id = self.sessions.create_record(record, permission)?;
                    self.owners.session_created(id, user)?;
                    aidl::write_create_session_reply(&mut reply, id);
                }
                Action::Open(id) => {
                    let (session, record) = self
                        .sessions
                        .records()
                        .into_iter()
                        .find(|(session, _)| session.id == id)
                        .ok_or_else(|| {
                            Exception::security(format!("Caller has no access to session {id}"))
                        })?;
                    if session.parameters.install_flags & (1 << 29) != 0
                        || !(uid == 0
                            || uid == session.installer_uid
                            || session.sealed && self.owners.is_verifier(uid)?)
                    {
                        return Err(Exception::security(format!(
                            "Caller has no access to session {id}"
                        )));
                    }
                    if !session.prepared {
                        self.owners.prepare_stage(&session, &record)?;
                        self.sessions.prepared(id)?;
                    }
                    let owners = self.owners.clone();
                    let node = Arc::new(SessionNode {
                        bound: self.sessions.bind_session(id)?,
                        operations: self.owners.session_operations(),
                        sessions: self.sessions.clone(),
                        id,
                        notify: Arc::new(move |event| owners.notify(event)),
                    });
                    let binder = self.owners.publish_session(node)?;
                    // Verifiers may open a sealed session without gaining mutable owner rights.
                    if let Some(event) = self.sessions.open_authorized(id)? {
                        self.owners.notify(event)
                    }
                    aidl::write_open_session_reply(&mut reply, Some(binder));
                }
                Action::Info(id) => {
                    let info = match self
                        .sessions
                        .records()
                        .into_iter()
                        .find(|(session, _)| session.id == id)
                    {
                        Some((session, record)) => self.info(&session, &record, true, uid)?,
                        None => None,
                    };
                    aidl::write_get_session_info_reply(&mut reply, info.as_ref());
                }
                Action::All(user) | Action::Mine(_, user) => {
                    self.owners.enforce_cross_user(
                        uid,
                        user,
                        if matches!(action, Action::Mine(..)) {
                            "getMySessions"
                        } else {
                            "getAllSessions"
                        },
                    )?;
                    let mine = if let Action::Mine(ref package, _) = action {
                        self.owners.check_package(uid, package.as_deref())?;
                        Some(package)
                    } else {
                        None
                    };
                    let mut items = Vec::new();
                    for (session, record) in self.sessions.records() {
                        if session.user as i32 != user || session.parent != -1 {
                            continue;
                        }
                        if let Some(package) = mine {
                            if &record.installer_package != package
                                || !(uid == 0 || uid == session.installer_uid)
                                || session.parameters.install_flags & (1 << 29) != 0
                            {
                                continue;
                            }
                            items.push(record.info(&session, false, 1000));
                        } else if let Some(info) = self.info(&session, &record, false, uid)? {
                            items.push(info)
                        }
                    }
                    let list = crate::shadow::ListSlice {
                        creator: "android.content.pm.PackageInstaller$SessionInfo".into(),
                        items,
                    };
                    if mine.is_some() {
                        aidl::write_get_my_sessions_reply(&mut reply, Some(&list));
                    } else {
                        aidl::write_get_all_sessions_reply(&mut reply, Some(&list));
                    }
                }
                Action::Staged => {
                    let mut items = Vec::new();
                    for (session, record) in self.sessions.records() {
                        if session.parameters.staged {
                            if let Some(info) = self.info(&session, &record, false, uid)? {
                                items.push(info)
                            }
                        }
                    }
                    let list = crate::shadow::ListSlice {
                        creator: "android.content.pm.PackageInstaller$SessionInfo".into(),
                        items,
                    };
                    aidl::write_get_staged_sessions_reply(&mut reply, Some(&list));
                }
                Action::Abandon(id) => {
                    let ids = self.sessions.abandon(id, uid)?;
                    self.owners.abandon_stage(&ids)?;
                    aidl::write_abandon_session_reply(&mut reply);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            reply = Parcel::new();
            reply.write_exception(&error)
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::Reader;
    struct UnreachableOwners;
    impl Owners for UnreachableOwners {
        fn uninstall(
            &self,
            _: super::super::removal::Request,
            _: Option<super::super::preapproval::IntentSender>,
        ) -> Result<(), Exception> {
            panic!("Unexpected uninstall")
        }
        fn archive(&self, _: u32, _: i32, _: ArchiveAction) -> Result<(), Exception> {
            panic!("Unexpected archive")
        }
        fn update_icon(
            &self,
            _: u32,
            _: i32,
            _: super::super::codec::Object,
        ) -> Result<(), Exception> {
            panic!("Unexpected icon")
        }
        fn install_constraints(
            &self,
            _: u32,
            _: Option<String>,
            _: Option<Vec<Option<String>>>,
            _: Option<super::super::constraints::Constraints>,
            _: ConstraintReceiver,
            _: i64,
        ) -> Result<(), Exception> {
            panic!("Unexpected constraints")
        }
        fn install_existing(
            &self,
            _: u32,
            _: i32,
            _: super::super::existing::Request,
        ) -> Result<(), Exception> {
            panic!("Unexpected existing install")
        }
        fn permissions_result(&self, _: u32, _: i32, _: bool) -> Result<(), Exception> {
            panic!("Unexpected permissions result")
        }
        fn normalize(
            &self,
            _: u32,
            _: i32,
            _: SessionParams,
            _: Option<String>,
            _: Option<String>,
            _: i32,
        ) -> Result<(Record, bool), Exception> {
            panic!("FD refusal must precede owner invocation")
        }
        fn session_created(&self, _: i32, _: u32) -> Result<(), Exception> {
            panic!("unexpected creation")
        }
        fn prepare_stage(&self, _: &Session, _: &Record) -> Result<(), Exception> {
            panic!("unexpected preparation")
        }
        fn session_operations(&self) -> Option<Arc<dyn super::super::SessionOperations>> {
            panic!("unexpected session owner")
        }
        fn publish_session(&self, _: Arc<SessionNode>) -> Result<Binder, Exception> {
            panic!("unexpected publication")
        }
        fn notify(&self, _: Event) {
            panic!("unexpected callback")
        }
        fn enforce_cross_user(&self, _: u32, _: i32, _: &str) -> Result<(), Exception> {
            panic!("unexpected policy")
        }
        fn check_package(&self, _: u32, _: Option<&str>) -> Result<(), Exception> {
            panic!("unexpected appops")
        }
        fn can_query(&self, _: u32, _: Option<&str>) -> Result<bool, Exception> {
            panic!("unexpected query")
        }
        fn can_read_paths(&self, _: u32) -> Result<bool, Exception> {
            panic!("unexpected path")
        }
        fn resolved_path(&self, _: i32) -> Result<Option<String>, Exception> {
            panic!("unexpected path")
        }
        fn is_verifier(&self, _: u32) -> Result<bool, Exception> {
            panic!("unexpected verifier")
        }
        fn update_label(&self, _: u32, _: i32, _: Option<String>) -> Result<(), Exception> {
            panic!("unexpected label update")
        }
        fn register_callback(&self, _: u32, _: Option<Binder>, _: i32) -> Result<(), Exception> {
            panic!("unexpected callback registration")
        }
        fn unregister_callback(&self, _: Option<Binder>) -> Result<(), Exception> {
            panic!("unexpected unregister")
        }
        fn set_service_policy(&self, _: u32, _: ServiceSetting) -> Result<(), Exception> {
            panic!("unexpected service policy")
        }
        fn abandon_stage(&self, _: &[i32]) -> Result<(), Exception> {
            panic!("unexpected abandon")
        }
    }
    #[test]
    fn create_refuses_unowned_fd_before_allocating_or_calling_policy() {
        let mut icon = Parcel::new();
        icon.write_string16(Some("android.graphics.Bitmap"));
        for value in [0, 4, 2, -1, 1, 1, 4, 160] {
            icon.write_i32(value);
        }
        icon.write_i64(3);
        icon.write_i32(1);
        icon.write_i32(4);
        icon.write_i32(1);
        icon.write_i32(0);
        let file = std::fs::File::open("/dev/null").unwrap();
        icon.write_file(
            aim_binder_host::server::file_from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap(),
        );
        icon.write_bool(false);
        let params = SessionParams {
            app_icon: Some(super::super::codec::Object {
                bytes: icon.data().to_vec(),
                objects: icon.objects().to_vec(),
            }),
            mode: 1,
            ..Default::default()
        };
        let mut data = Parcel::new();
        aidl::CreateSession {
            params: Some(params),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }
        .write(&mut data);
        let sessions = Arc::new(Sessions::default());
        let endpoint = Endpoint {
            sessions: sessions.clone(),
            owners: Arc::new(UnreachableOwners),
        };
        let reply = endpoint
            .transact(&mut Call {
                code: aidl::CREATE_SESSION,
                flags: 0,
                sender_pid: 1,
                sender_euid: 10100,
                data: Reader::new(data.data(), data.objects()),
            })
            .unwrap();
        assert_eq!(
            aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap_err()
                .code,
            EX_UNSUPPORTED_OPERATION
        );
        assert!(sessions.records().is_empty());
    }
}
