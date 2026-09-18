//! Long-lived, peer-authenticated Binder routing control sessions.
//! Payload storage remains in each process-local endpoint.

use crate::{binder_transfer::TransferTable, ProfileError};
use darwin_art_binder_device::{
    authority_protocol::{self, ConnectionToken, Message, TransferToken},
    routing_authority::{Outbound, PeerIdentity, RoutingAuthority, Session},
};
use std::{
    collections::HashMap,
    io,
    net::Shutdown,
    os::{
        fd::OwnedFd,
        unix::net::UnixStream,
    },
    sync::{
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread,
};

const OUTBOUND_CAPACITY: usize = 128;

struct Endpoint {
    peer: PeerIdentity,
    session: Session,
    sender: SyncSender<Message>,
    shutdown: Arc<UnixStream>,
}

/// One routed transfer removed from the service table. It can be sent once or
/// dropped; neither outcome can put the descriptor back under the old token.
pub(crate) struct TransferDelivery(Vec<OwnedFd>);

impl TransferDelivery {
    pub(crate) fn into_descriptors(self) -> Vec<OwnedFd> {
        self.0
    }

}

#[derive(Default)]
pub(crate) struct BinderService {
    authority: RoutingAuthority,
    endpoints: Mutex<HashMap<ConnectionToken, Endpoint>>,
    transfers: Mutex<TransferTable>,
}

impl BinderService {
    pub(crate) fn serve(
        &self,
        mut stream: UnixStream,
        peer: PeerIdentity,
    ) -> Result<(), ProfileError> {
        let writer_stream = stream.try_clone()?;
        let shutdown = Arc::new(stream.try_clone()?);
        let writer_shutdown = Arc::clone(&shutdown);
        let (sender, receiver) = mpsc::sync_channel(OUTBOUND_CAPACITY);
        let opened = {
            let mut endpoints = self.endpoints.lock().map_err(|_| poisoned())?;
            if endpoints.values().any(|endpoint| endpoint.peer == peer) {
                return Err(ProfileError::Daemon(
                    "Binder peer already has an active session".into(),
                ));
            }
            endpoints
                .try_reserve(1)
                .map_err(|_| ProfileError::Daemon("Binder endpoint table is full".into()))?;
            let opened = self.authority.open_authenticated(peer).map_err(failed)?;
            endpoints.insert(
                opened.session.connection(),
                Endpoint {
                    peer,
                    session: opened.session.clone(),
                    sender: sender.clone(),
                    shutdown: Arc::clone(&shutdown),
                },
            );
            opened
        };
        let connection = opened.session.connection();
        let writer = thread::spawn(move || {
            let mut stream = writer_stream;
            while let Ok(message) = receiver.recv() {
                if authority_protocol::encode(&mut stream, message).is_err() {
                    let _ = writer_shutdown.shutdown(Shutdown::Both);
                    break;
                }
            }
        });
        self.enqueue(&sender, opened.response)?;

        let result = loop {
            let message = match authority_protocol::decode(&mut stream) {
                Ok(message) => message,
                Err(authority_protocol::DecodeError::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::BrokenPipe
                    ) =>
                {
                    break Ok(());
                }
                Err(error) => break Err(protocol_failed(error)),
            };
            match self.handle_message(&opened.session, &sender, message) {
                Ok(true) => break Ok(()),
                Ok(false) => {}
                Err(error) => break Err(error),
            }
        };

        self.disconnect(connection);
        let _ = shutdown.shutdown(Shutdown::Both);
        drop(sender);
        let _ = writer.join();
        result
    }

    fn handle_message(
        &self,
        session: &Session,
        sender: &SyncSender<Message>,
        message: Message,
    ) -> Result<bool, ProfileError> {
        match message {
            Message::PublishNode { local } => {
                let response = self
                    .authority
                    .publish_node(session, local)
                    .map_err(failed)?;
                self.enqueue(sender, response)?;
            }
            Message::SetContextManager { local } => {
                let response = self
                    .authority
                    .set_context_manager(session, local)
                    .map_err(failed)?;
                self.enqueue(sender, response)?;
            }
            Message::GetContextManager => {
                let response = self
                    .authority
                    .get_context_manager(session)
                    .map_err(failed)?;
                self.enqueue(sender, response)?;
            }
            Message::RouteTransaction {
                target,
                caller_thread,
                transfer,
                code,
                flags,
            } => {
                let mut transfers = self.transfers.lock().map_err(|_| poisoned())?;
                transfers
                    .ensure_pending(session.connection(), transfer)
                    .map_err(transfer_failed)?;
                let outcome = match self.authority.route_transaction(
                    session,
                    target,
                    caller_thread,
                    transfer,
                    code,
                    flags,
                ) {
                    Ok(outcome) => outcome,
                    Err(
                        error @ (darwin_art_binder_device::routing_authority::Error::DeadTarget
                        | darwin_art_binder_device::routing_authority::Error::UnknownNode),
                    ) => {
                        transfers
                            .discard_pending(session.connection(), transfer)
                            .map_err(transfer_failed)?;
                        drop(transfers);
                        self.enqueue(
                            sender,
                            Message::RouteRejected {
                                reason: if matches!(
                                    error,
                                    darwin_art_binder_device::routing_authority::Error::DeadTarget
                                ) {
                                    darwin_art_binder_device::authority_protocol::TransactionFailure::DeadReply
                                } else {
                                    darwin_art_binder_device::authority_protocol::TransactionFailure::FailedReply
                                },
                            },
                        )?;
                        return Ok(false);
                    }
                    Err(error) => return Err(failed(error)),
                };
                transfers
                    .route(session.connection(), transfer, outcome.delivery.destination)
                    .map_err(transfer_failed)?;
                drop(transfers);
                self.enqueue(sender, outcome.acknowledgement)?;
                self.dispatch(outcome.delivery);
            }
            Message::CompleteReply {
                call,
                transfer,
                code,
                flags,
            } => {
                let mut transfers = self.transfers.lock().map_err(|_| poisoned())?;
                transfers
                    .ensure_pending(session.connection(), transfer)
                    .map_err(transfer_failed)?;
                let outbound = self
                    .authority
                    .complete_reply(session, call, transfer, code, flags)
                    .map_err(failed)?;
                if let Some(outbound) = outbound {
                    transfers
                        .route(session.connection(), transfer, outbound.destination)
                        .map_err(transfer_failed)?;
                    drop(transfers);
                    self.enqueue(sender, Message::ReplyAccepted { call })?;
                    self.dispatch(outbound);
                } else {
                    transfers
                        .discard_pending(session.connection(), transfer)
                        .map_err(transfer_failed)?;
                    drop(transfers);
                    self.enqueue(sender, Message::ReplyAccepted { call })?;
                }
            }
            Message::RequestDeath { target, cookie } => {
                let response = self
                    .authority
                    .request_death(session, target, cookie)
                    .map_err(failed)?;
                self.enqueue(sender, response)?;
            }
            Message::ClearDeath { target, cookie } => {
                let response = self
                    .authority
                    .clear_death(session, target, cookie)
                    .map_err(failed)?;
                self.enqueue(sender, response)?;
            }
            Message::CloseConnection => return Ok(true),
            Message::OpenConnection
            | Message::ConnectionOpened { .. }
            | Message::NodePublished { .. }
            | Message::ContextManagerSet { .. }
            | Message::ContextManagerFound { .. }
            | Message::RouteAccepted { .. }
            | Message::RouteRejected { .. }
            | Message::ReplyAccepted { .. }
            | Message::DeathRequested
            | Message::DeathCleared
            | Message::DeliverTransaction { .. }
            | Message::DeliverReply { .. }
            | Message::TargetDead { .. }
            | Message::CallerDead { .. }
            | Message::NodeDead { .. } => {
                return Err(ProfileError::Daemon(
                    "invalid Binder message direction".into(),
                ));
            }
        }
        Ok(false)
    }

    fn enqueue(&self, sender: &SyncSender<Message>, message: Message) -> Result<(), ProfileError> {
        // The writer is the sole owner of socket egress. A full in-memory
        // outbox is backpressure, not evidence that the Android process died.
        // Client request registration never holds its response mutex across a
        // socket write, so waiting here cannot block the dispatcher that drains
        // this outbox.
        sender
            .send(message)
            .map_err(|_| ProfileError::Daemon("Binder peer writer disconnected".into()))
    }

    fn dispatch(&self, first: Outbound) {
        let mut pending = vec![first];
        while let Some(outbound) = pending.pop() {
            let sender = self.endpoints.lock().ok().and_then(|endpoints| {
                endpoints
                    .get(&outbound.destination)
                    .map(|endpoint| endpoint.sender.clone())
            });
            let delivered = sender
                .as_ref()
                .is_some_and(|sender| sender.send(outbound.message).is_ok());
            if !delivered {
                pending.extend(self.remove(outbound.destination));
            }
        }
    }

    fn disconnect(&self, connection: ConnectionToken) {
        for notification in self.remove(connection) {
            self.dispatch(notification);
        }
    }

    pub(crate) fn deposit(
        &self,
        peer: PeerIdentity,
        token: TransferToken,
        descriptors: Vec<OwnedFd>,
    ) -> Result<(), ProfileError> {
        let connection = self.connection_for_peer(peer)?;
        self.transfers
            .lock()
            .map_err(|_| poisoned())?
            .deposit(connection, token, descriptors)
            .map_err(transfer_failed)
    }

    /// Receives exactly one immutable transfer carrier from the authenticated
    /// peer and deposits it into the peer's active Binder connection. Keeping
    /// recvmsg and table publication in one owner prevents a generic daemon
    /// handler from accidentally retaining or duplicating the descriptor.
    pub(crate) fn receive_deposit(
        &self,
        peer: PeerIdentity,
        token: TransferToken,
        stream: &UnixStream,
    ) -> Result<(), ProfileError> {
        let descriptors = crate::fd_passing::receive_many(stream)?;
        self.deposit(peer, token, descriptors)
    }

    pub(crate) fn take(
        &self,
        peer: PeerIdentity,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<Vec<OwnedFd>, ProfileError> {
        let destination = self.connection_for_peer(peer)?;
        self.transfers
            .lock()
            .map_err(|_| poisoned())?
            .take(destination, source, token)
            .map_err(transfer_failed)
    }

    /// Consumes one routed bundle for the authenticated destination. The
    /// caller hands it to the host delivery owner before native enqueue;
    /// this one-shot Binder token cannot be replayed on a delivery failure.
    pub(crate) fn prepare_take(
        &self,
        peer: PeerIdentity,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<TransferDelivery, ProfileError> {
        self.take(peer, source, token).map(TransferDelivery)
    }

    fn connection_for_peer(&self, peer: PeerIdentity) -> Result<ConnectionToken, ProfileError> {
        let endpoints = self.endpoints.lock().map_err(|_| poisoned())?;
        let mut matches = endpoints
            .iter()
            .filter_map(|(connection, endpoint)| (endpoint.peer == peer).then_some(*connection));
        let connection = matches
            .next()
            .ok_or_else(|| ProfileError::Daemon("Binder peer has no active session".into()))?;
        if matches.next().is_some() {
            return Err(ProfileError::Daemon(
                "Binder peer has multiple active sessions".into(),
            ));
        }
        Ok(connection)
    }

    fn remove(&self, connection: ConnectionToken) -> Vec<Outbound> {
        let endpoint = self
            .endpoints
            .lock()
            .ok()
            .and_then(|mut endpoints| endpoints.remove(&connection));
        let Some(endpoint) = endpoint else {
            return Vec::new();
        };
        let _ = endpoint.shutdown.shutdown(Shutdown::Both);
        if let Ok(mut transfers) = self.transfers.lock() {
            transfers.remove_connection(connection);
        }
        self.authority
            .close(&endpoint.session)
            .map(|outcome| outcome.notifications)
            .unwrap_or_default()
    }
}

fn failed(error: darwin_art_binder_device::routing_authority::Error) -> ProfileError {
    ProfileError::Daemon(format!(
        "Binder routing authority rejected request: {error:?}"
    ))
}

fn protocol_failed(error: authority_protocol::DecodeError) -> ProfileError {
    ProfileError::Daemon(format!("invalid Binder authority protocol: {error:?}"))
}

fn transfer_failed(error: crate::binder_transfer::Error) -> ProfileError {
    ProfileError::Daemon(format!("Binder transfer rejected: {error:?}"))
}

fn poisoned() -> ProfileError {
    ProfileError::Daemon("Binder endpoint table is poisoned".into())
}

#[cfg(test)]
mod tests;
