use crate::{AuthorityTransport, Error, client};
use darwin_art_binder_device::{authority_protocol::Message, remote_transaction};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::{env, os::fd::IntoRawFd};

pub type ProcessDispatcher = Dispatcher<darwin_art_profile::BinderAuthorityConnection>;

pub struct Dispatcher<T: AuthorityTransport> {
    state: Arc<client::State<T>>,
}

impl<T: AuthorityTransport> Dispatcher<T> {
    pub(crate) fn new(state: Arc<client::State<T>>) -> Self {
        Self { state }
    }

    /// Exactly one process-owned thread calls this loop. It is the sole reader
    /// of the authority stream; Binder worker threads never race to consume a
    /// response intended for another request.
    pub fn run(&self) -> Result<(), Error> {
        loop {
            self.dispatch_next()?;
        }
    }

    pub fn dispatch_next(&self) -> Result<(), Error> {
        let result = self.dispatch_next_inner();
        if let Err(error) = &result {
            self.fail_pending(format!("authority dispatcher stopped: {error:?}"));
        }
        result
    }

    fn dispatch_next_inner(&self) -> Result<(), Error> {
        let message = self
            .state
            .transport
            .receive()
            .map_err(|error| Error::Transport(error.0))?;
        match message {
            Message::NodePublished { .. }
            | Message::ContextManagerSet { .. }
            | Message::ContextManagerFound { .. }
            | Message::RouteAccepted { .. }
            | Message::RouteRejected { .. }
            | Message::ReplyAccepted { .. } => self.complete_request(message),
            Message::DeathRequested | Message::DeathCleared => self.complete_request(message),
            Message::DeliverTransaction {
                call,
                sender,
                sender_pid,
                sender_euid,
                target,
                transfer,
                code,
                flags,
                ..
            } => {
                if env::var_os("DARWIN_ART_DEBUG_BINDER").is_some() {
                    eprintln!(
                        "ART Binder authority: deliver transaction call={call:?} sender={sender:?} target={target:?} code={code} flags=0x{flags:x}"
                    );
                }
                let mut image = self
                    .state
                    .transport
                    .take_transfer(sender, transfer)
                    .map_err(|error| Error::Transport(error.0))?;
                let files = self.install_received_fds(&mut image)?;
                self.state
                    .device
                    .deliver_remote_transaction_with_fds(
                        &image,
                        self.state.transport.connection(),
                        remote_transaction::RemoteTransaction {
                            call,
                            sender,
                            sender_pid,
                            sender_euid,
                            target,
                            code,
                            flags,
                        },
                        files,
                    )
                    .map(|_| ())
                    .map_err(device_error)
            }
            Message::DeliverReply {
                call,
                source,
                sender_pid,
                sender_euid,
                target_thread,
                transfer,
                code,
                flags,
            } => {
                if env::var_os("DARWIN_ART_DEBUG_BINDER").is_some() {
                    eprintln!(
                        "ART Binder authority: deliver reply call={call:?} source={source:?} target_thread={target_thread} code={code} flags=0x{flags:x}"
                    );
                }
                let mut image = self
                    .state
                    .transport
                    .take_transfer(source, transfer)
                    .map_err(|error| Error::Transport(error.0))?;
                let files = self.install_received_fds(&mut image)?;
                if env::var_os("DARWIN_ART_DEBUG_BINDER").is_some() {
                    let status = image
                        .data()
                        .get(..4)
                        .map(|bytes| i32::from_le_bytes(bytes.try_into().unwrap()));
                    eprintln!(
                        "ART Binder authority: reply payload bytes={} status_word={status:?}",
                        image.data().len()
                    );
                }
                self.state
                    .device
                    .deliver_remote_reply_with_fds(
                        &image,
                        self.state.transport.connection(),
                        remote_transaction::RemoteReply {
                            call,
                            source,
                            sender_pid,
                            sender_euid,
                            target_thread,
                            code,
                            flags,
                        },
                        files,
                    )
                    .map(|_| ())
                    .map_err(device_error)
            }
            Message::TargetDead {
                call,
                target_thread,
            } => self
                .state
                .device
                .deliver_remote_target_dead(target_thread, call)
                .map_err(device_error),
            Message::CallerDead { call } => self
                .state
                .device
                .deliver_remote_caller_dead(call)
                .map_err(device_error),
            Message::NodeDead { cookie } => self
                .state
                .device
                .enqueue_remote_dead_binder(cookie)
                .map_err(device_error),
            _ => Err(Error::Protocol(
                "authority sent a client-only or duplicate-open message".into(),
            )),
        }
    }

    fn install_received_fds(
        &self,
        image: &mut darwin_art_binder_device::transfer_image::TransferImage,
    ) -> Result<Vec<darwin_art_binder_device::installed_fds::InstalledFd>, Error> {
        let transferred = image.take_files();
        if transferred.is_empty() {
            return Ok(Vec::new());
        }
        let api = self
            .state
            .descriptor_api
            .lock()
            .map_err(|_| Error::Poisoned)?
            .ok_or_else(|| Error::Transfer("Binder FD transport is not installed".into()))?;
        let mut files = Vec::new();
        files
            .try_reserve_exact(transferred.len())
            .map_err(|_| Error::Transfer("Binder FD ownership allocation failed".into()))?;
        for (offset, descriptor) in transferred {
            let end = offset
                .checked_add(24)
                .ok_or_else(|| Error::Transfer("Binder FD object offset overflow".into()))?;
            let source = image
                .data()
                .get(offset..end)
                .ok_or_else(|| Error::Transfer("Binder FD object is outside Parcel data".into()))?;
            let mut object = [0_u8; 24];
            object.copy_from_slice(source);
            let number = unsafe { (api.import)(descriptor.into_raw_fd()) };
            if number < 0 {
                return Err(Error::Transfer(
                    "Binder FD could not be installed in receiver namespace".into(),
                ));
            }
            let number = u32::try_from(number)
                .map_err(|_| Error::Transfer("Binder FD number is outside Android ABI".into()))?;
            object[8..12].copy_from_slice(&number.to_le_bytes());
            files.push(darwin_art_binder_device::installed_fds::InstalledFd::new(
                offset,
                object,
                number,
                move || {
                    let _ = unsafe { (api.close)(number as i32) };
                },
            ));
        }
        Ok(files)
    }

    fn complete_request(&self, message: Message) -> Result<(), Error> {
        let pending = self
            .state
            .pending
            .lock()
            .map_err(|_| Error::Poisoned)?
            .pop_front()
            .ok_or_else(|| Error::Protocol("authority response has no pending request".into()))?;
        if !client::matches_expected(pending.expected, message) {
            let reason = format!(
                "authority response {:?} does not match {:?}",
                message, pending.expected
            );
            let _ = pending.response.send(Err(Error::Protocol(reason.clone())));
            return Err(Error::Protocol(reason));
        }
        if let Some(commit) = pending.remote_commit {
            let result = match (commit, message) {
                (
                    client::RemoteCommit::ContextManager { reservation },
                    Message::ContextManagerSet { .. },
                ) => self.state.device.commit_context_manager(reservation),
                (
                    client::RemoteCommit::Transaction {
                        thread_id,
                        submission,
                    },
                    Message::RouteAccepted { call },
                ) => self
                    .state
                    .device
                    .commit_remote_submission(thread_id, submission, call),
                (
                    client::RemoteCommit::Transaction {
                        thread_id,
                        submission,
                    },
                    Message::RouteRejected { reason },
                ) => self
                    .state
                    .device
                    .reject_remote_submission(thread_id, submission, reason),
                (
                    client::RemoteCommit::Reply {
                        thread_id,
                        submission,
                        call,
                    },
                    Message::ReplyAccepted { call: accepted },
                ) if call == accepted => self
                    .state
                    .device
                    .commit_remote_reply(thread_id, submission, call),
                _ => {
                    let reason =
                        "authority acknowledgement does not match pending commit".to_owned();
                    let _ = pending.response.send(Err(Error::Protocol(reason.clone())));
                    return Err(Error::Protocol(reason));
                }
            };
            if let Err(error) = result {
                let reason = format!("failed to commit accepted remote call: {error:?}");
                let _ = pending.response.send(Err(Error::Device(reason.clone())));
                return Err(Error::Device(reason));
            }
        }
        pending
            .response
            .send(Ok(message))
            .map_err(|_| Error::Closed)
    }

    fn fail_pending(&self, reason: String) {
        if self.state.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(mut pending) = self.state.pending.lock() else {
            return;
        };
        while let Some(request) = pending.pop_front() {
            let _ = request.response.send(Err(Error::Protocol(reason.clone())));
        }
        drop(pending);
        let _ = self.state.device.fail_remote_authority();
    }
}

fn device_error<E: std::fmt::Debug>(error: E) -> Error {
    Error::Device(format!("{error:?}"))
}
