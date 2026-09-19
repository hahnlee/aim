use crate::{AuthorityTransport, Dispatcher};
use darwin_art_binder_device::{
    authority_protocol::{CallToken, LocalNodeToken, Message, NodeToken, TransferToken},
    descriptor_manifest::DescriptorBundle,
    device::{ContextManagerReservation, Device, OpenConnection, ProcessIdentity},
    thread::RemoteSubmission,
    transaction_snapshot::TransactionSnapshot,
    transfer_image::TransferImage,
};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const MAX_PENDING_REQUESTS: usize = 128;
const SLOW_BINDER_REQUEST: Duration = Duration::from_millis(50);

#[cfg(test)]
#[path = "retained_deposit_tests.rs"]
mod retained_deposit_tests;

pub type ProcessClient = Client<darwin_art_profile::BinderAuthorityConnection>;

#[derive(Debug)]
pub enum Error {
    Closed,
    Poisoned,
    Busy,
    Protocol(String),
    Transport(String),
    Device(String),
    Transfer(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmissionOutcome {
    Accepted(Option<CallToken>),
    Rejected(darwin_art_binder_device::authority_protocol::TransactionFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Expected {
    NodePublished,
    ContextManagerSet,
    ContextManagerFound,
    RouteResult,
    ReplyAccepted,
    DeathRequested,
    DeathCleared,
}

pub(crate) struct Pending {
    pub expected: Expected,
    pub response: mpsc::SyncSender<Result<Message, Error>>,
    pub remote_commit: Option<RemoteCommit>,
}

#[derive(Clone, Copy)]
pub(crate) enum RemoteCommit {
    ContextManager {
        reservation: ContextManagerReservation,
    },
    Transaction {
        thread_id: u64,
        submission: RemoteSubmission,
    },
    Reply {
        thread_id: u64,
        submission: darwin_art_binder_device::thread::RemoteReplySubmission,
        call: CallToken,
    },
}

pub(crate) struct State<T: AuthorityTransport> {
    pub transport: Arc<T>,
    pub device: Arc<OpenConnection>,
    pub pending: Mutex<VecDeque<Pending>>,
    /// Serializes request registration with wire emission without blocking the
    /// sole response dispatcher on `pending`.
    send_order: Mutex<()>,
    pub closed: Arc<AtomicBool>,
    next_transfer: Mutex<u64>,
    exported_nodes: Mutex<HashMap<LocalNodeToken, ExportedNode>>,
    pub descriptor_api: Mutex<Option<crate::DescriptorApi>>,
}

#[derive(Default)]
struct ExportedNode {
    token: Option<NodeToken>,
    strong: Option<darwin_art_binder_device::node_owner::LocalHold>,
    weak: Option<darwin_art_binder_device::node_owner::LocalHold>,
}

pub struct Client<T: AuthorityTransport> {
    pub(crate) state: Arc<State<T>>,
}

impl<T: AuthorityTransport> Drop for State<T> {
    fn drop(&mut self) {
        // Metadata leases must become terminal before the actual transport
        // and device fields are destroyed, even without an explicit close.
        self.closed.store(true, Ordering::Release);
    }
}

struct SubmissionRequest<'a> {
    thread_id: u64,
    target: NodeToken,
    snapshot: &'a TransactionSnapshot,
    extra: &'a [u8],
    objects: &'a [darwin_art_binder_device::remote_objects::OutboundObject],
    code: u32,
    flags: u32,
}

impl<T: AuthorityTransport> Clone for Client<T> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

pub fn connect(
    socket: &Path,
    pid: i32,
) -> Result<(ProcessClient, crate::ProcessDispatcher), Error> {
    let transport = darwin_art_profile::connect_binder_authority_at(socket)
        .map_err(|error| Error::Transport(error.to_string()))?;
    let identity = ProcessIdentity::new(pid, transport.android_uid())
        .ok_or_else(|| Error::Device("invalid Binder process identity".into()))?;
    let device = Device::default()
        .open(identity)
        .map_err(|error| Error::Device(format!("{error:?}")))?;
    Ok(Client::new(transport, device))
}

pub fn connect_process(
    socket: &Path,
    pid: i32,
) -> Result<(ProcessClient, crate::ProcessDispatcher), Error> {
    connect(socket, pid)
}

impl<T: AuthorityTransport> Client<T> {
    pub fn new(transport: T, device: OpenConnection) -> (Self, Dispatcher<T>) {
        let state = Arc::new(State {
            transport: Arc::new(transport),
            device: Arc::new(device),
            pending: Mutex::new(VecDeque::with_capacity(MAX_PENDING_REQUESTS)),
            send_order: Mutex::new(()),
            closed: Arc::new(AtomicBool::new(false)),
            next_transfer: Mutex::new(0),
            exported_nodes: Mutex::new(HashMap::new()),
            descriptor_api: Mutex::new(None),
        });
        (
            Self {
                state: Arc::clone(&state),
            },
            Dispatcher::new(state),
        )
    }

    pub fn device(&self) -> &OpenConnection {
        &self.state.device
    }

    pub fn authority_lifetime(&self) -> crate::AuthorityLifetime {
        crate::AuthorityLifetime {
            closed: Arc::clone(&self.state.closed),
        }
    }

    pub(crate) fn install_descriptor_api(&self, api: crate::DescriptorApi) -> Result<(), Error> {
        let mut installed = self.descriptor_api()?;
        if installed.is_some() {
            return Err(Error::Busy);
        }
        *installed = Some(api);
        Ok(())
    }

    /// End the process-owned authority stream. The sole dispatcher observes
    /// EOF after the daemon consumes this message; no Binder worker reads the
    /// control stream directly.
    pub fn close_authority(&self) -> Result<(), Error> {
        let result = self
            .state
            .transport
            .send(Message::CloseConnection)
            .map_err(|error| Error::Transport(error.0));
        if !self.state.closed.swap(true, Ordering::AcqRel) {
            let _ = self.state.device.fail_remote_authority();
        }
        result
    }

    pub fn request_death(&self, target: NodeToken, cookie: u64) -> Result<(), Error> {
        match self.request(
            Message::RequestDeath { target, cookie },
            Expected::DeathRequested,
            None,
        )? {
            Message::DeathRequested => Ok(()),
            _ => unreachable!("response kind checked by dispatcher"),
        }
    }

    pub fn clear_death(&self, target: NodeToken, cookie: u64) -> Result<(), Error> {
        match self.request(
            Message::ClearDeath { target, cookie },
            Expected::DeathCleared,
            None,
        )? {
            Message::DeathCleared => Ok(()),
            _ => unreachable!("response kind checked by dispatcher"),
        }
    }

    pub fn publish_node(&self, local: LocalNodeToken) -> Result<NodeToken, Error> {
        match self.request(
            Message::PublishNode { local },
            Expected::NodePublished,
            None,
        )? {
            Message::NodePublished { node } => Ok(node),
            _ => unreachable!("response kind checked by dispatcher"),
        }
    }

    pub fn set_context_manager(&self, local: LocalNodeToken) -> Result<NodeToken, Error> {
        match self.request(
            Message::SetContextManager { local },
            Expected::ContextManagerSet,
            None,
        )? {
            Message::ContextManagerSet { node } => Ok(node),
            _ => unreachable!("response kind checked by dispatcher"),
        }
    }

    /// Execute the original SET_CONTEXT_MGR ioctl as one distributed
    /// operation. The local binder_proc node remains reserved until the sole
    /// authority dispatcher receives ContextManagerSet.
    pub fn set_context_manager_control(
        &self,
        request: u64,
        argument: &[u8],
    ) -> Result<NodeToken, Error> {
        let (reservation, local) = self
            .state
            .device
            .begin_context_manager_control::<()>(request, argument)
            .map_err(device_error)?;
        let result = self.request(
            Message::SetContextManager { local },
            Expected::ContextManagerSet,
            Some(RemoteCommit::ContextManager { reservation }),
        );
        match result {
            Ok(Message::ContextManagerSet { node }) => Ok(node),
            Ok(_) => unreachable!("response kind checked by dispatcher"),
            Err(error) => {
                self.state.device.abort_context_manager(reservation);
                Err(error)
            }
        }
    }

    pub fn get_context_manager(&self) -> Result<Option<NodeToken>, Error> {
        let node = match self.request(
            Message::GetContextManager,
            Expected::ContextManagerFound,
            None,
        )? {
            Message::ContextManagerFound { node } => node,
            _ => unreachable!("response kind checked by dispatcher"),
        };
        if let Some(node) = node {
            let handle = self
                .state
                .device
                .install_remote_context_manager(
                    node,
                    darwin_art_binder_device::reference_table::Strength::Strong,
                )
                .map_err(device_error)?;
            if handle != 0 {
                return Err(Error::Protocol(format!(
                    "authority context manager installed at handle {handle}, not handle 0"
                )));
            }
        }
        Ok(node)
    }

    pub fn install_remote_reference(
        &self,
        node: NodeToken,
        strength: darwin_art_binder_device::reference_table::Strength,
    ) -> Result<u32, Error> {
        self.state
            .device
            .install_remote_reference(node, strength)
            .map_err(device_error)
    }

    pub fn submit_transaction(
        &self,
        thread_id: u64,
        target: NodeToken,
        snapshot: &TransactionSnapshot,
        extra: &[u8],
        code: u32,
        flags: u32,
    ) -> Result<SubmissionOutcome, Error> {
        let objects = self
            .state
            .device
            .resolve_remote_objects(snapshot)
            .map_err(device_error)?;
        self.submit_transaction_with_objects(
            thread_id, target, snapshot, extra, &objects, code, flags,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn submit_transaction_with_objects(
        &self,
        thread_id: u64,
        target: NodeToken,
        snapshot: &TransactionSnapshot,
        extra: &[u8],
        objects: &[darwin_art_binder_device::remote_objects::OutboundObject],
        code: u32,
        flags: u32,
    ) -> Result<SubmissionOutcome, Error> {
        let synchronous = flags & 1 == 0;
        let submission = self
            .state
            .device
            .begin_remote_submission(thread_id, synchronous)
            .map_err(device_error)?;
        let request = SubmissionRequest {
            thread_id,
            target,
            snapshot,
            extra,
            objects,
            code,
            flags,
        };
        let result = self.submit_reserved(submission, request);
        if result.is_err() {
            let _ = self
                .state
                .device
                .abort_remote_submission(thread_id, submission);
        }
        result
    }

    pub fn submit_remote_command(
        &self,
        thread_id: u64,
        command: &darwin_art_binder_device::device::RemoteCommand,
    ) -> Result<SubmissionOutcome, Error> {
        self.submit_transaction_with_objects(
            thread_id,
            command.target(),
            command.snapshot(),
            command.extra(),
            command.objects(),
            command.code(),
            command.flags(),
        )
    }

    fn submit_reserved(
        &self,
        submission: RemoteSubmission,
        request: SubmissionRequest<'_>,
    ) -> Result<SubmissionOutcome, Error> {
        let started = Instant::now();
        let manifest = self.publish_remote_objects(request.objects)?;
        let objects_published = Instant::now();
        // Allocate before descriptor export: a typed provider grant must bind
        // the actual export to this transfer, not to a subsequently chosen ID.
        let transfer = self.allocate_transfer()?;
        let (files, retained_leases) = self.capture_remote_fds(request.snapshot, transfer)?;
        let mut transfer_receipt = files
            .iter()
            .any(|file| !file.attributes().as_bytes().is_empty())
            .then(|| {
                crate::transfer_receipts::Source::new(self.state.transport.as_ref(), transfer)
            });
        let image = TransferImage::capture_with_objects_and_descriptors(
            request.snapshot,
            request.extra,
            &manifest,
            files,
        )
        .map_err(|error| Error::Transfer(format!("{error:?}")))?;
        let transfer_captured = Instant::now();
        self.state
            .transport
            .deposit_transfer(transfer, &image)
            .map_err(|error| Error::Transport(error.0))?;
        // The daemon's installed-ownership ACK ends source export retention;
        // routing is a separate Binder policy operation.
        drop(retained_leases);
        let transfer_deposited = Instant::now();
        let response = self.request(
            Message::RouteTransaction {
                target: request.target,
                caller_thread: request.thread_id,
                transfer,
                code: request.code,
                flags: request.flags,
            },
            Expected::RouteResult,
            Some(RemoteCommit::Transaction {
                thread_id: request.thread_id,
                submission,
            }),
        )?;
        let route_accepted = Instant::now();
        if std::env::var_os("DARWIN_ART_DEBUG_BINDER_SLOW").is_some()
            && route_accepted.duration_since(started) >= SLOW_BINDER_REQUEST
        {
            eprintln!(
                "ART Binder slow outbound: thread={} target={:?} code={} flags=0x{:x} objects_us={} capture_us={} deposit_us={} route_us={} total_us={}",
                request.thread_id,
                request.target,
                request.code,
                request.flags,
                objects_published.duration_since(started).as_micros(),
                transfer_captured
                    .duration_since(objects_published)
                    .as_micros(),
                transfer_deposited
                    .duration_since(transfer_captured)
                    .as_micros(),
                route_accepted
                    .duration_since(transfer_deposited)
                    .as_micros(),
                route_accepted.duration_since(started).as_micros(),
            );
        }
        match response {
            Message::RouteAccepted { call } => {
                if let Some(receipt) = &mut transfer_receipt {
                    receipt.routed();
                }
                Ok(SubmissionOutcome::Accepted(call))
            }
            Message::RouteRejected { reason } => Ok(SubmissionOutcome::Rejected(reason)),
            _ => unreachable!("response kind checked by dispatcher"),
        }
    }

    pub fn complete_remote_reply(
        &self,
        thread_id: u64,
        command: &darwin_art_binder_device::device::RemoteReplyCommand,
    ) -> Result<(), Error> {
        let result = self.complete_remote_reply_reserved(thread_id, command);
        if result.is_err() {
            let _ = self.state.device.abort_remote_reply(
                thread_id,
                command.submission(),
                command.call(),
            );
        }
        result
    }

    fn complete_remote_reply_reserved(
        &self,
        thread_id: u64,
        command: &darwin_art_binder_device::device::RemoteReplyCommand,
    ) -> Result<(), Error> {
        let manifest = self.publish_remote_objects(command.objects())?;
        let transfer = self.allocate_transfer()?;
        let (files, retained_leases) = self.capture_remote_fds(command.snapshot(), transfer)?;
        let mut transfer_receipt = files
            .iter()
            .any(|file| !file.attributes().as_bytes().is_empty())
            .then(|| {
                crate::transfer_receipts::Source::new(self.state.transport.as_ref(), transfer)
            });
        let image = TransferImage::capture_with_objects_and_descriptors(
            command.snapshot(),
            command.extra(),
            &manifest,
            files,
        )
        .map_err(|error| Error::Transfer(format!("{error:?}")))?;
        self.state
            .transport
            .deposit_transfer(transfer, &image)
            .map_err(|error| Error::Transport(error.0))?;
        drop(retained_leases);
        let response = self.request(
            Message::CompleteReply {
                call: command.call(),
                transfer,
                code: command.code(),
                flags: command.flags(),
            },
            Expected::ReplyAccepted,
            Some(RemoteCommit::Reply {
                thread_id,
                submission: command.submission(),
                call: command.call(),
            }),
        )?;
        let Message::ReplyAccepted { call } = response else {
            unreachable!("response kind checked by dispatcher")
        };
        if call != command.call() {
            return Err(Error::Protocol(
                "reply acknowledgement call mismatch".into(),
            ));
        }
        if let Some(receipt) = &mut transfer_receipt {
            receipt.routed();
        }
        Ok(())
    }

    fn allocate_transfer(&self) -> Result<TransferToken, Error> {
        let mut serial = self.next_transfer()?;
        *serial = serial.checked_add(1).ok_or(Error::Closed)?;
        TransferToken::from_nonzero(*serial).ok_or(Error::Closed)
    }

    fn capture_remote_fds(
        &self,
        snapshot: &TransactionSnapshot,
        transfer: TransferToken,
    ) -> Result<
        (
            Vec<DescriptorBundle>,
            Vec<crate::descriptor_transport::RetainedDescriptorLease>,
        ),
        Error,
    > {
        use darwin_art_binder_device::object_fields::Fields;
        let parsed = snapshot
            .objects()
            .map_err(|error| Error::Transfer(format!("{error:?}")))?;
        let count = parsed
            .iter()
            .filter(|object| matches!(object.fields(), Fields::Fd { .. }))
            .count();
        if count == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        // Copy the function table out before invoking a provider. No install
        // mutex is held across an export (or any provider-side RPC).
        let api = self
            .descriptor_api()?
            .as_ref()
            .copied()
            .ok_or_else(|| Error::Transfer("Binder FD transport is not installed".into()))?;
        let mut files = Vec::new();
        let mut retained_leases = Vec::new();
        files
            .try_reserve_exact(count)
            .map_err(|_| Error::Transfer("Binder FD manifest allocation failed".into()))?;
        if api.retained.is_some() {
            retained_leases
                .try_reserve_exact(count)
                .map_err(|_| Error::Transfer("Binder FD lease allocation failed".into()))?;
        }
        for object in parsed {
            let Fields::Fd { fd, .. } = object.fields() else {
                continue;
            };
            let binding = crate::DescriptorTransferBinding::new(
                self.state.transport.connection().get(),
                transfer.get(),
                files.len() as u64,
                object.offset(),
            );
            if api.retained.is_some() {
                let (file, lease) = api
                    .export_bound_retained(fd as i32, binding)
                    .map_err(Error::Transfer)?;
                files.push(file);
                retained_leases.push(lease);
            } else {
                files.push(
                    api.export_bound(fd as i32, binding)
                        .map_err(Error::Transfer)?,
                );
            }
        }
        Ok((files, retained_leases))
    }

    fn descriptor_api(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Option<crate::DescriptorApi>>, Error> {
        self.state
            .descriptor_api
            .lock()
            .map_err(|_| Error::Poisoned)
    }

    fn publish_remote_objects(
        &self,
        objects: &[darwin_art_binder_device::remote_objects::OutboundObject],
    ) -> Result<Vec<darwin_art_binder_device::remote_objects::ManifestObject>, Error> {
        use darwin_art_binder_device::remote_objects::{ManifestObject, OutboundSource};
        let mut manifest = Vec::new();
        manifest
            .try_reserve_exact(objects.len())
            .map_err(|_| Error::Transfer("remote object manifest allocation failed".into()))?;
        for object in objects {
            let node = match object.source {
                OutboundSource::Local(local) => {
                    self.publish_exported_node(local, object.strength)?
                }
                OutboundSource::Remote(node) => node,
            };
            manifest.push(ManifestObject {
                offset: object.offset,
                node,
                strength: object.strength,
                flags: object.flags,
            });
        }
        Ok(manifest)
    }

    fn publish_exported_node(
        &self,
        local: LocalNodeToken,
        strength: darwin_art_binder_device::reference_table::Strength,
    ) -> Result<NodeToken, Error> {
        use darwin_art_binder_device::reference_table::Strength;

        // Serialize first publication per local binder node. The held device
        // reference schedules the same owner BR_* lifecycle work as the Linux
        // binder driver before the Parcel that contains the flat object can be
        // released by its sending thread.
        let mut exports = self
            .state
            .exported_nodes
            .lock()
            .map_err(|_| Error::Poisoned)?;
        let entry = exports.entry(local).or_default();
        match strength {
            Strength::Strong if entry.strong.is_none() => {
                entry.strong = Some(
                    self.state
                        .device
                        .hold_exported_node(local, Strength::Strong)
                        .map_err(device_error)?,
                );
                entry.weak = None;
            }
            Strength::Weak if entry.strong.is_none() && entry.weak.is_none() => {
                entry.weak = Some(
                    self.state
                        .device
                        .hold_exported_node(local, Strength::Weak)
                        .map_err(device_error)?,
                );
            }
            Strength::Strong | Strength::Weak => {}
        }
        if let Some(node) = entry.token {
            return Ok(node);
        }
        match self.publish_node(local) {
            Ok(node) => {
                entry.token = Some(node);
                Ok(node)
            }
            Err(error) => {
                exports.remove(&local);
                Err(error)
            }
        }
    }

    fn next_transfer(&self) -> Result<std::sync::MutexGuard<'_, u64>, Error> {
        self.state.next_transfer.lock().map_err(|_| Error::Poisoned)
    }

    fn request(
        &self,
        message: Message,
        expected: Expected,
        remote_commit: Option<RemoteCommit>,
    ) -> Result<Message, Error> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let _send_order = self.state.send_order.lock().map_err(|_| Error::Poisoned)?;
        if self.state.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let mut pending = self.state.pending.lock().map_err(|_| Error::Poisoned)?;
        if pending.len() == MAX_PENDING_REQUESTS {
            return Err(Error::Busy);
        }
        pending.push_back(Pending {
            expected,
            response: sender,
            remote_commit,
        });
        // The response dispatcher must be able to consume an acknowledgement
        // while this thread is backpressured writing the next request.
        drop(pending);
        if let Err(error) = self.state.transport.send(message) {
            if !self.state.closed.swap(true, Ordering::AcqRel) {
                let mut pending = self.state.pending.lock().map_err(|_| Error::Poisoned)?;
                while let Some(request) = pending.pop_front() {
                    let _ = request.response.send(Err(Error::Closed));
                }
                drop(pending);
                let _ = self.state.device.fail_remote_authority();
            }
            return Err(Error::Transport(error.0));
        }
        drop(_send_order);
        receiver.recv().map_err(|_| Error::Closed)?
    }
}

fn device_error<E: std::fmt::Debug>(error: E) -> Error {
    Error::Device(format!("{error:?}"))
}

pub(crate) fn matches_expected(expected: Expected, message: Message) -> bool {
    matches!(
        (expected, message),
        (Expected::NodePublished, Message::NodePublished { .. })
            | (
                Expected::ContextManagerSet,
                Message::ContextManagerSet { .. }
            )
            | (
                Expected::ContextManagerFound,
                Message::ContextManagerFound { .. }
            )
            | (Expected::RouteResult, Message::RouteAccepted { .. })
            | (Expected::RouteResult, Message::RouteRejected { .. })
            | (Expected::ReplyAccepted, Message::ReplyAccepted { .. })
            | (Expected::DeathRequested, Message::DeathRequested)
            | (Expected::DeathCleared, Message::DeathCleared)
    )
}
