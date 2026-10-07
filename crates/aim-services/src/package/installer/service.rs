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
        #[derive(Clone)]
        enum Action {
            Set(f32),
            Add(f32),
            Close,
            AddChild(i32),
            RemoveChild(i32),
            Read,
            Names,
            OpenRead(Option<String>),
            RemoveSplit(Option<String>),
            Abandon,
            Loader,
            Seal,
            Transfer(Option<String>),
        }
        let action = match call.code {
            aidl::TRANSFER => Action::Transfer(aidl::Transfer::read(&mut call.data)?.package_name),
            aidl::SEAL => {
                aidl::Seal::read(&mut call.data)?;
                Action::Seal
            }
            aidl::GET_NAMES => {
                aidl::GetNames::read(&mut call.data)?;
                Action::Names
            }
            aidl::OPEN_READ => Action::OpenRead(aidl::OpenRead::read(&mut call.data)?.name),
            aidl::REMOVE_SPLIT => {
                Action::RemoveSplit(aidl::RemoveSplit::read(&mut call.data)?.split_name)
            }
            aidl::ABANDON => {
                aidl::Abandon::read(&mut call.data)?;
                Action::Abandon
            }
            aidl::GET_DATA_LOADER_PARAMS => {
                aidl::GetDataLoaderParams::read(&mut call.data)?;
                Action::Loader
            }
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
        let result = match action.clone() {
            Action::Transfer(name) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.transfer_session(self.id, call.sender_euid, name)),
            Action::Seal => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.seal_session(self.id, call.sender_euid)),
            Action::Names => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.names(self.id, call.sender_euid))
                .map(|names| aidl::write_get_names_reply(&mut reply, &Some(names))),
            Action::OpenRead(name) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.open_read(self.id, call.sender_euid, name))
                .map(|file| {
                    reply.write_no_exception();
                    reply.write_i32(1);
                    reply.write_i32(0);
                    reply.write_file(file);
                }),
            Action::RemoveSplit(name) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.remove_split(self.id, call.sender_euid, name)),
            Action::Abandon => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.abandon_session(self.id, call.sender_euid)),
            Action::Loader => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.data_loader(self.id, call.sender_euid))
                .map(|value| {
                    reply.write_no_exception();
                    if let Some(value) = value {
                        let mut reader =
                            aim_binder_host::parcel::Reader::new(&value.bytes, &value.objects);
                        reader.read_string16().unwrap();
                        reply.write_i32(1);
                        reply.write_raw(&value.bytes[reader.position()..], &[]);
                    } else {
                        reply.write_i32(0);
                    }
                }),
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
            Action::AddChild(id) => self
                .sessions
                .add_child(self.id, id, call.sender_euid)
                .and_then(|()| {
                    self.operations
                        .as_ref()
                        .map_or(Ok(()), |owner| owner.graph_changed())
                }),
            Action::RemoveChild(id) => self
                .sessions
                .remove_child(self.id, id, call.sender_euid)
                .and_then(|()| {
                    self.operations
                        .as_ref()
                        .map_or(Ok(()), |owner| owner.graph_changed())
                }),
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
                if !matches!(
                    action,
                    Action::Read | Action::Names | Action::OpenRead(_) | Action::Loader
                ) {
                    reply.write_no_exception();
                }
            }
            Err(error) => reply.write_exception(&error),
        }
        Ok(reply)
    }
}

fn missing() -> Exception {
    Exception::new(
        aim_binder_host::parcel::EX_ILLEGAL_STATE,
        "Native session storage owner unavailable",
    )
}
