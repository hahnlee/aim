use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_UNSUPPORTED_OPERATION, Exception, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_content_pm_ipackageinstallersession as aidl;

use super::{INSTALL_REQUEST_UPDATE_OWNERSHIP, SessionNode};

impl Service for SessionNode {
    fn accepts_fds(&self) -> bool {
        true
    }
    fn descriptor(&self) -> &str {
        aidl::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // Decode before mutation: trailing bytes must not change state.
        enum Action {
            Commit(Option<super::preapproval::IntentSender>, bool),
            Preapproval(super::codec::Object),
            HardLink(Option<String>),
            Checksums(
                Option<String>,
                Option<Vec<Option<super::checksums::Checksum>>>,
                Option<Vec<u8>>,
            ),
            RequestChecksums(
                Option<String>,
                i32,
                i32,
                super::checksums::TrustedInstallers,
                Option<aim_binder_host::parcel::Binder>,
            ),
            FetchNames,
            AddFile(super::InstallationFile),
            RemoveFile(i32, Option<String>),
            DomainsRead,
            DomainsWrite(Option<crate::package::domain_verification::domain_set::DomainSet>),
            Set(f32),
            Add(f32),
            Close,
            AddChild(i32),
            RemoveChild(i32),
            Read,
            Names,
            OpenRead(Option<String>),
            OpenWrite(Option<String>, i64, i64),
            Write(Option<String>, i64, i64, Option<u32>),
            RemoveSplit(Option<String>),
            Abandon,
            Loader,
            MetadataRead,
            MetadataWrite,
            MetadataRemove,
            Seal,
            Transfer(Option<String>),
        }
        let action = match call.code {
            aidl::COMMIT => {
                let args = aidl::Commit::<super::preapproval::IntentSender>::read(&mut call.data)?;
                Action::Commit(args.status_receiver, args.for_transferred)
            }
            aidl::REQUEST_USER_PREAPPROVAL => {
                call.data.enforce_interface(aidl::DESCRIPTOR)?;
                let start = call.data.position();
                call.data.skip(call.data.remaining())?;
                let (bytes, objects) = call.data.since(start);
                Action::Preapproval(super::codec::Object {
                    bytes: bytes.to_vec(),
                    objects,
                })
            }
            aidl::STAGE_VIA_HARD_LINK => {
                Action::HardLink(aidl::StageViaHardLink::read(&mut call.data)?.target)
            }
            aidl::SET_CHECKSUMS => {
                let args = aidl::SetChecksums::<super::checksums::Checksum>::read(&mut call.data)?;
                Action::Checksums(args.name, args.checksums, args.signature)
            }
            aidl::REQUEST_CHECKSUMS => {
                call.data.enforce_interface(aidl::DESCRIPTOR)?;
                let name = call.data.read_string16()?;
                let optional = call.data.read_i32()?;
                let required = call.data.read_i32()?;
                let trusted = super::checksums::TrustedInstallers::read(&mut call.data)?;
                let listener = call.data.read_binder()?;
                Action::RequestChecksums(name, optional, required, trusted, listener)
            }
            aidl::FETCH_PACKAGE_NAMES => {
                aidl::FetchPackageNames::read(&mut call.data)?;
                Action::FetchNames
            }
            aidl::ADD_FILE => {
                let args = aidl::AddFile::read(&mut call.data)?;
                Action::AddFile(super::InstallationFile {
                    location: args.location,
                    name: args.name,
                    length: args.length_bytes,
                    metadata: args.metadata,
                    signature: args.signature,
                })
            }
            aidl::REMOVE_FILE => {
                let args = aidl::RemoveFile::read(&mut call.data)?;
                Action::RemoveFile(args.location, args.name)
            }
            aidl::GET_PRE_VERIFIED_DOMAINS => {
                aidl::GetPreVerifiedDomains::read(&mut call.data)?;
                Action::DomainsRead
            }
            aidl::SET_PRE_VERIFIED_DOMAINS => Action::DomainsWrite(
                aidl::SetPreVerifiedDomains::read(&mut call.data)?.pre_verified_domains,
            ),
            aidl::GET_APP_METADATA_FD => {
                aidl::GetAppMetadataFd::read(&mut call.data)?;
                Action::MetadataRead
            }
            aidl::OPEN_WRITE_APP_METADATA => {
                aidl::OpenWriteAppMetadata::read(&mut call.data)?;
                Action::MetadataWrite
            }
            aidl::REMOVE_APP_METADATA => {
                aidl::RemoveAppMetadata::read(&mut call.data)?;
                Action::MetadataRemove
            }
            aidl::TRANSFER => Action::Transfer(aidl::Transfer::read(&mut call.data)?.package_name),
            aidl::SEAL => {
                aidl::Seal::read(&mut call.data)?;
                Action::Seal
            }
            aidl::GET_NAMES => {
                aidl::GetNames::read(&mut call.data)?;
                Action::Names
            }
            aidl::OPEN_WRITE => {
                let a = aidl::OpenWrite::read(&mut call.data)?;
                Action::OpenWrite(a.name, a.offset_bytes, a.length_bytes)
            }
            aidl::WRITE => {
                let a = aidl::Write::<IncomingFd>::read(&mut call.data)?;
                Action::Write(a.name, a.offset_bytes, a.length_bytes, a.fd.map(|fd| fd.0))
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
        let returns_value = matches!(
            &action,
            Action::FetchNames
                | Action::DomainsRead
                | Action::Read
                | Action::Names
                | Action::OpenRead(_)
                | Action::OpenWrite(..)
                | Action::MetadataRead
                | Action::MetadataWrite
                | Action::Loader
        );
        let result = match action {
            Action::Commit(receiver, transferred) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| {
                    owner.commit_session(self.id, call.sender_euid, receiver, transferred)
                }),
            Action::Preapproval(body) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.request_preapproval(self.id, call.sender_euid, body)),
            Action::HardLink(target) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.stage_hard_link(self.id, call.sender_euid, target)),
            Action::Checksums(name, checksums, signature) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| {
                    owner.set_checksums(self.id, call.sender_euid, name, checksums, signature)
                }),
            Action::RequestChecksums(name, optional, required, trusted, listener) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| {
                    owner.request_checksums(
                        self.id,
                        call.sender_euid,
                        name,
                        optional,
                        required,
                        trusted,
                        listener,
                    )
                }),
            Action::FetchNames => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.fetch_package_names(self.id, call.sender_euid))
                .map(|names| aidl::write_fetch_package_names_reply(&mut reply, &Some(names))),
            Action::AddFile(file) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.add_file(self.id, call.sender_euid, file)),
            Action::RemoveFile(location, name) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.remove_file(self.id, call.sender_euid, location, name)),
            Action::DomainsWrite(domains) => {
                self.operations
                    .as_ref()
                    .ok_or_else(missing)
                    .and_then(|owner| {
                        owner.set_pre_verified_domains(self.id, call.sender_euid, domains)
                    })
            }
            Action::DomainsRead => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.pre_verified_domains(self.id, call.sender_euid))
                .map(|domains| {
                    reply.write_no_exception();
                    if let Some(domains) = domains {
                        reply.write_i32(1);
                        reply.write_bool(false);
                        reply.write_i32(domains.len() as i32);
                        for domain in domains {
                            reply.write_string16(Some(&domain));
                        }
                    } else {
                        reply.write_i32(0);
                    }
                }),
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
            Action::MetadataRead => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.metadata_read(self.id, call.sender_euid))
                .map(|file| {
                    reply.write_no_exception();
                    match file {
                        None => reply.write_i32(0),
                        Some(file) => {
                            reply.write_i32(1);
                            reply.write_i32(0);
                            reply.write_file(file);
                        }
                    }
                }),
            Action::MetadataWrite => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.metadata_write(self.id, call.sender_euid))
                .map(|file| {
                    reply.write_no_exception();
                    reply.write_i32(1);
                    reply.write_i32(0);
                    reply.write_file(file);
                }),
            Action::MetadataRemove => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.metadata_remove(self.id)),
            Action::Names => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.names(self.id, call.sender_euid))
                .map(|names| aidl::write_get_names_reply(&mut reply, &Some(names))),
            Action::OpenWrite(name, offset, length) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| owner.open_write(self.id, call.sender_euid, name, offset, length))
                .map(|file| {
                    reply.write_no_exception();
                    reply.write_i32(1);
                    reply.write_i32(0);
                    reply.write_file(file);
                }),
            Action::Write(name, offset, length, fd) => self
                .operations
                .as_ref()
                .ok_or_else(missing)
                .and_then(|owner| {
                    owner.write_file(self.id, call.sender_euid, name, offset, length, fd)
                }),
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
                if !returns_value {
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

struct IncomingFd(u32);
impl aim_service_aidl::ReadParcelable for IncomingFd {
    fn read_from(
        reader: &mut aim_binder_host::parcel::Reader<'_>,
    ) -> aim_binder_host::parcel::Result<Self> {
        let comm = reader.read_i32()?;
        let fd = reader.read_fd()?;
        if comm != 0 {
            reader.read_fd()?;
        }
        Ok(Self(fd))
    }
}
