//! Device-open resource registration. No packet PID or integer is a lookup key.
use crate::{
    connection::{self, Connection, ConnectionOwner, TargetConnection},
    session::{self, transaction_target::TargetReferences},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
mod authority;
pub mod context_manager;
mod lifecycle;
mod process_io;
#[cfg(target_os = "macos")]
mod remote_delivery;

/// Opaque local capability; transport code must keep it on its authenticated
/// channel, never deserialize a client-supplied registry id into this type.
#[derive(Clone)]
pub struct Key {
    registry: Arc<()>,
    id: crate::routing_id::ConnectionId,
}
#[derive(Default)]
struct Entries {
    next: u64,
    owners: HashMap<crate::routing_id::ConnectionId, ConnectionOwner>,
}
#[derive(Default)]
pub struct Registry {
    identity: Arc<()>,
    entries: Mutex<Entries>,
    context: Mutex<context_manager::State>,
    transaction_gate: Mutex<()>,
    #[cfg(target_os = "macos")]
    calls: Mutex<authority::CallTable>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    ContextSelfAcquire,
    ForeignRegistry,
    UnknownConnection,
    Exhausted,
    OutOfMemory,
    Poisoned,
    Connection(connection::Error),
}

#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
pub enum SubmitError {
    Registry(Error),
    Context(context_manager::RoutingError),
    ReplyRequiresThreadStack,
    Snapshot(crate::objects::Error),
    Target(connection::Error),
    CallIdsExhausted,
    OutOfMemory,
    UnknownCall,
}
#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
pub enum DeliveryError {
    Registry(Error),
    Thread(crate::thread::Error),
}
impl Registry {
    pub(crate) fn fail_remote_authority(&self, key: &Key) -> Result<(), Error> {
        self.lookup(key)?
            .fail_remote_authority()
            .map_err(Error::Connection)
    }

    pub(crate) fn hold_local_node(
        &self,
        key: &Key,
        local: crate::authority_protocol::LocalNodeToken,
        kind: crate::node_owner::HoldKind,
    ) -> Result<crate::node_owner::LocalHold, Error> {
        self.lookup(key)?
            .hold_local_node(local, kind)
            .map_err(Error::Connection)
    }

    pub fn execute(&self, key: &Key, input: &[u8]) -> Result<session::Outcome, Error> {
        let (decoded, _) = crate::command::decode(input).map_err(|e| {
            Error::Connection(connection::Error::Command(session::Error::Framing(e)))
        })?;
        use crate::command::Kind;
        if matches!(
            decoded.kind,
            Kind::Increfs | Kind::Acquire | Kind::Release | Kind::Decrefs
        ) && u32::from_le_bytes(decoded.payload[..4].try_into().unwrap()) == 0
        {
            return self.execute_context_reference(key, input, decoded.kind);
        }
        self.lookup(key)?.execute(input).map_err(Error::Connection)
    }

    pub(crate) fn read_node_work(&self, key: &Key, output: &mut [u8]) -> Result<usize, Error> {
        self.lookup(key)?
            .read_node_work(output)
            .map_err(Error::Connection)
    }

    pub(crate) fn read_death_work(&self, key: &Key, output: &mut [u8]) -> Result<usize, Error> {
        self.lookup(key)?
            .read_death_work(output)
            .map_err(Error::Connection)
    }

    pub(crate) fn remote_node_for_handle(
        &self,
        key: &Key,
        handle: u32,
    ) -> Result<crate::authority_protocol::NodeToken, Error> {
        self.lookup(key)?
            .remote_node_for_handle(handle)
            .map_err(Error::Connection)
    }

    pub(crate) fn enqueue_dead_binder(&self, key: &Key, cookie: u64) -> Result<(), Error> {
        self.lookup(key)?
            .enqueue_dead_binder(cookie)
            .map_err(Error::Connection)
    }

    pub(crate) fn enqueue_clear_death_done(&self, key: &Key, cookie: u64) -> Result<(), Error> {
        self.lookup(key)?
            .enqueue_clear_death_done(cookie)
            .map_err(Error::Connection)
    }

    pub(crate) fn acknowledge_dead_binder(&self, key: &Key, cookie: u64) -> Result<(), Error> {
        self.lookup(key)?
            .acknowledge_dead_binder(cookie)
            .map_err(Error::Connection)
    }

    pub fn bind_target(
        &self,
        key: &Key,
        references: TargetReferences,
    ) -> Result<TargetConnection, Error> {
        self.lookup(key)?
            .bind_target(references)
            .map_err(Error::Connection)
    }

    fn bind_resolved_target(
        &self,
        references: TargetReferences,
    ) -> Result<(Key, TargetConnection), Error> {
        let id = references
            .node()
            .owner_connection()
            .ok_or(Error::UnknownConnection)?;
        let target = {
            let entries = self.entries.lock().map_err(|_| Error::Poisoned)?;
            entries
                .owners
                .get(&id)
                .map(ConnectionOwner::handle)
                .ok_or(Error::UnknownConnection)?
        };
        let target = target.bind_target(references).map_err(Error::Connection)?;
        Ok((
            Key {
                registry: Arc::clone(&self.identity),
                id,
            },
            target,
        ))
    }

    fn key_for_target(&self, target: &TargetConnection) -> Result<Key, Error> {
        let id = target
            .node()
            .owner_connection()
            .ok_or(Error::UnknownConnection)?;
        let key = Key {
            registry: Arc::clone(&self.identity),
            id,
        };
        self.lookup(&key)?;
        Ok(key)
    }

    #[cfg(target_os = "macos")]
    pub fn submit_transaction(
        &self,
        sender: &Key,
        request: &crate::transaction_request::Request,
        snapshot: &crate::transaction_snapshot::TransactionSnapshot,
        sender_pid: i32,
        sender_euid: u32,
        reservation: crate::thread::SubmissionReservation<'_>,
    ) -> Result<usize, SubmitError> {
        let _gate = self
            .transaction_gate
            .lock()
            .map_err(|_| SubmitError::Registry(Error::Poisoned))?;
        if request.target() == crate::transaction_request::Target::Reply {
            return Err(SubmitError::ReplyRequiresThreadStack);
        }
        snapshot.objects().map_err(SubmitError::Snapshot)?;
        let sender_connection = self.lookup(sender).map_err(SubmitError::Registry)?;
        let target = if request.target() == crate::transaction_request::Target::Handle(0) {
            self.resolve_context_target(sender, request)
                .map_err(SubmitError::Context)?
        } else {
            let references = sender_connection
                .resolve_transaction_target(request)
                .map_err(SubmitError::Target)?;
            self.bind_resolved_target(references)
                .map_err(SubmitError::Registry)?
                .1
        };
        self.key_for_target(&target)
            .map_err(SubmitError::Registry)?;
        let synchronous = request.flags() & 1 == 0;
        let call = if synchronous {
            let mut calls = self
                .calls
                .lock()
                .map_err(|_| SubmitError::Registry(Error::Poisoned))?;
            let call = calls.register(authority::CallRoute {
                sender: sender.id,
                thread_id: reservation.thread_id(),
                target: target
                    .node()
                    .routing_id()
                    .ok_or(SubmitError::Registry(Error::UnknownConnection))?,
            })?;
            Some(call)
        } else {
            None
        };
        let node = target.node();
        let result = target
            .enqueue_translated_transaction(
                &sender_connection,
                snapshot,
                crate::transaction_queue::Header {
                    reply: false,
                    target_pointer: node.pointer(),
                    target_cookie: node.cookie(),
                    code: request.code(),
                    flags: request.flags(),
                    sender_pid,
                    sender_euid,
                },
                crate::transaction_queue::Route {
                    call,
                    target_thread: None,
                    deferred_completion: false,
                },
            )
            .map_err(SubmitError::Target);
        match result {
            Ok(address) => {
                reservation.commit(call);
                Ok(address)
            }
            Err(error) => {
                if let Some(call) = call {
                    self.calls
                        .lock()
                        .map_err(|_| SubmitError::Registry(Error::Poisoned))?
                        .remove(call);
                }
                Err(error)
            }
        }
    }

    #[cfg(target_os = "macos")]
    pub fn submit_reply(
        &self,
        replier: &Key,
        call: crate::thread::CallId,
        request: &crate::transaction_request::Request,
        snapshot: &crate::transaction_snapshot::TransactionSnapshot,
        sender: crate::write_read::SenderIdentity,
        reservation: crate::thread::SubmissionReservation<'_>,
    ) -> Result<usize, SubmitError> {
        let _gate = self
            .transaction_gate
            .lock()
            .map_err(|_| SubmitError::Registry(Error::Poisoned))?;
        if request.target() != crate::transaction_request::Target::Reply {
            return Err(SubmitError::ReplyRequiresThreadStack);
        }
        let route = self
            .calls
            .lock()
            .map_err(|_| SubmitError::Registry(Error::Poisoned))?
            .get(call)
            .ok_or(SubmitError::UnknownCall)?;
        let replier_connection = self.lookup(replier).map_err(SubmitError::Registry)?;
        let caller_connection = match self.lookup_id(route.sender) {
            Ok(connection) => connection,
            Err(Error::UnknownConnection) => {
                // Preserve calls after caller teardown until the callee either
                // replies or dies. That makes a legitimate late BC_REPLY
                // distinguishable from a forged/stale transaction stack.
                self.calls
                    .lock()
                    .map_err(|_| SubmitError::Registry(Error::Poisoned))?
                    .remove(call);
                reservation.commit(None);
                return Ok(0);
            }
            Err(error) => return Err(SubmitError::Registry(error)),
        };
        let result = caller_connection
            .enqueue_translated_from(
                &replier_connection,
                snapshot,
                crate::transaction_queue::Header {
                    reply: true,
                    target_pointer: 0,
                    target_cookie: 0,
                    code: request.code(),
                    flags: request.flags(),
                    sender_pid: sender.pid,
                    sender_euid: sender.euid,
                },
                crate::transaction_queue::Route {
                    call: Some(call),
                    target_thread: Some(route.thread_id),
                    deferred_completion: true,
                },
            )
            .map_err(SubmitError::Target);
        match result {
            Ok(address) => {
                self.calls
                    .lock()
                    .map_err(|_| SubmitError::Registry(Error::Poisoned))?
                    .remove(call);
                reservation.commit(None);
                Ok(address)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests;
