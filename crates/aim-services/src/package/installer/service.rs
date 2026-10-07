use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_UNSUPPORTED_OPERATION, Exception, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_content_pm_ipackageinstallersession as aidl;

use super::{INSTALL_REQUEST_UPDATE_OWNERSHIP, SessionNode};

impl Service for SessionNode {
    fn descriptor(&self) -> &str {
        aidl::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // Decode before mutation: trailing bytes must not change state.
        #[derive(Clone, Copy)]
        enum Action {
            Set(f32),
            Add(f32),
            Close,
            AddChild(i32),
            RemoveChild(i32),
            Read,
        }
        let action = match call.code {
            aidl::SET_CLIENT_PROGRESS => {
                Action::Set(aidl::SetClientProgress::read(&mut call.data)?.progress)
            }
            aidl::ADD_CLIENT_PROGRESS => {
                Action::Add(aidl::AddClientProgress::read(&mut call.data)?.progress)
            }
            aidl::CLOSE => {
                aidl::Close::read(&mut call.data)?;
                Action::Close
            }
            aidl::ADD_CHILD_SESSION_ID => {
                Action::AddChild(aidl::AddChildSessionId::read(&mut call.data)?.session_id)
            }
            aidl::REMOVE_CHILD_SESSION_ID => {
                Action::RemoveChild(aidl::RemoveChildSessionId::read(&mut call.data)?.session_id)
            }
            aidl::IS_MULTI_PACKAGE => {
                aidl::IsMultiPackage::read(&mut call.data)?;
                Action::Read
            }
            aidl::GET_CHILD_SESSION_IDS => {
                aidl::GetChildSessionIds::read(&mut call.data)?;
                Action::Read
            }
            aidl::GET_PARENT_SESSION_ID => {
                aidl::GetParentSessionId::read(&mut call.data)?;
                Action::Read
            }
            aidl::IS_STAGED => {
                aidl::IsStaged::read(&mut call.data)?;
                Action::Read
            }
            aidl::GET_INSTALL_FLAGS => {
                aidl::GetInstallFlags::read(&mut call.data)?;
                Action::Read
            }
            aidl::IS_APPLICATION_ENABLED_SETTING_PERSISTENT => {
                aidl::IsApplicationEnabledSettingPersistent::read(&mut call.data)?;
                Action::Read
            }
            aidl::IS_REQUEST_UPDATE_OWNERSHIP => {
                aidl::IsRequestUpdateOwnership::read(&mut call.data)?;
                Action::Read
            }
            _ => {
                if aidl::METHODS.iter().any(|&(code, _)| code == call.code) {
                    call.data.enforce_interface(aidl::DESCRIPTOR)?;
                    let mut reply = Parcel::new();
                    reply.write_exception(&Exception::new(
                        EX_UNSUPPORTED_OPERATION,
                        "Native install storage, commit and callback owners are not connected",
                    ));
                    return Ok(reply);
                }
                return Err(UNKNOWN_TRANSACTION);
            }
        };
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        let mut reply = Parcel::new();
        let result = match action {
            Action::Set(value) => self
                .sessions
                .progress(self.id, call.sender_euid, value, false)
                .map(|event| {
                    if let Some(event) = event {
                        (self.notify)(event);
                    }
                }),
            Action::Add(value) => self
                .sessions
                .progress(self.id, call.sender_euid, value, true)
                .map(|event| {
                    if let Some(event) = event {
                        (self.notify)(event);
                    }
                }),
            Action::Close => self.sessions.close(self.id, call.sender_euid).map(|event| {
                if let Some(event) = event {
                    (self.notify)(event);
                }
            }),
            Action::AddChild(id) => self.sessions.add_child(self.id, id, call.sender_euid),
            Action::RemoveChild(id) => self.sessions.remove_child(self.id, id, call.sender_euid),
            Action::Read => self
                .sessions
                .snapshot(self.id)
                .map(|session| match call.code {
                    aidl::IS_MULTI_PACKAGE => aidl::write_is_multi_package_reply(
                        &mut reply,
                        session.parameters.multi_package,
                    ),
                    aidl::GET_CHILD_SESSION_IDS => aidl::write_get_child_session_ids_reply(
                        &mut reply,
                        &Some(session.children.iter().copied().collect()),
                    ),
                    aidl::GET_PARENT_SESSION_ID => {
                        aidl::write_get_parent_session_id_reply(&mut reply, session.parent)
                    }
                    aidl::IS_STAGED => {
                        aidl::write_is_staged_reply(&mut reply, session.parameters.staged)
                    }
                    aidl::GET_INSTALL_FLAGS => aidl::write_get_install_flags_reply(
                        &mut reply,
                        session.parameters.install_flags,
                    ),
                    aidl::IS_APPLICATION_ENABLED_SETTING_PERSISTENT => {
                        aidl::write_is_application_enabled_setting_persistent_reply(
                            &mut reply,
                            session.parameters.application_enabled_setting_persistent,
                        )
                    }
                    aidl::IS_REQUEST_UPDATE_OWNERSHIP => {
                        aidl::write_is_request_update_ownership_reply(
                            &mut reply,
                            session.parameters.install_flags & INSTALL_REQUEST_UPDATE_OWNERSHIP
                                != 0,
                        )
                    }
                    _ => unreachable!(),
                }),
        };
        match result {
            Ok(()) => {
                if !matches!(action, Action::Read) {
                    reply.write_no_exception();
                }
            }
            Err(error) => reply.write_exception(&error),
        }
        Ok(reply)
    }
}
