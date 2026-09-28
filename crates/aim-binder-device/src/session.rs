//! Resources of one opened Binder device connection (kernel binder_proc scope).
//! Transport authentication and device registration must own this value; it is
//! not selected from sender-provided PID fields and is not a framework service.
use crate::{
    command::{self, Kind},
    node_ack,
    node_owner::{self, Node, NodeOwner},
    objects::Object,
    reference_command,
    reference_table::{self, Counts, ReferenceTable, Strength},
};
use std::{io, sync::Arc, time::Duration};
pub mod context_manager;
pub mod context_reference;
pub mod transaction_target;

#[derive(Default)]
pub struct Session {
    // Drop owner first: local nodes become dead before outgoing refs disappear.
    owner: NodeOwner,
    pub(crate) references: ReferenceTable,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Framing(command::DecodeError),
    Unsupported(Kind),
    Reference(reference_command::Error),
    Acknowledgement(node_ack::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    ContextReference(context_reference::Outcome),
    Reference(reference_command::Outcome),
    Acknowledgement(node_ack::Outcome),
}
impl Outcome {
    pub fn bytes(&self) -> usize {
        match self {
            Self::ContextReference(result) => result.bytes,
            Self::Reference(result) => result.bytes,
            Self::Acknowledgement(result) => result.bytes,
        }
    }
}

impl Session {
    pub(crate) fn attach_work_signal(&self, signal: Arc<crate::work_signal::Signal>) {
        self.owner.attach_work_signal(signal);
    }

    pub(crate) fn bind_routing_id(&self, id: crate::routing_id::ConnectionId) -> Result<(), ()> {
        self.owner.bind_routing_id(id)
    }

    pub(crate) fn owns_node(&self, node: &Node) -> bool {
        self.owner.owns(node)
    }
    /// Only pass a node object copied/validated from this connection's sender.
    pub fn resolve_local(&mut self, object: &Object<'_>) -> Result<Arc<Node>, node_owner::Error> {
        self.owner.resolve(object)
    }

    pub fn lookup_local_node(
        &self,
        token: crate::authority_protocol::LocalNodeToken,
    ) -> Result<Arc<Node>, node_owner::Error> {
        self.owner.lookup_local(token)
    }

    /// Transaction execution must authorize transfer before retaining the node.
    /// This resource operation is not itself a cross-process security check.
    pub fn retain_transferred(
        &mut self,
        node: Arc<Node>,
        strength: Strength,
    ) -> Result<(u32, Counts, Counts), reference_table::Error> {
        self.references.retain(node, strength)
    }

    pub fn retain_remote(
        &mut self,
        node: crate::authority_protocol::NodeToken,
        strength: Strength,
    ) -> Result<(u32, Counts, Counts), reference_table::Error> {
        self.references.retain_remote(node, strength)
    }

    pub fn retain_remote_context_manager(
        &mut self,
        node: crate::authority_protocol::NodeToken,
        strength: Strength,
    ) -> Result<(u32, Counts, Counts), reference_table::Error> {
        self.references
            .retain_remote_context_manager(node, strength)
    }

    /// Execute exactly one supported record against this connection's resources.
    /// Unsupported transactions/looper/death commands never report success.
    /// This is not a complete BINDER_WRITE_READ implementation.
    pub fn execute(&mut self, input: &[u8]) -> Result<Outcome, Error> {
        let (decoded, _) = command::decode(input).map_err(Error::Framing)?;
        match decoded.kind {
            Kind::Increfs | Kind::Acquire | Kind::Release | Kind::Decrefs => {
                reference_command::dispatch(&mut self.references, input)
                    .map(Outcome::Reference)
                    .map_err(Error::Reference)
            }
            Kind::IncrefsDone | Kind::AcquireDone => node_ack::dispatch(&self.owner, input)
                .map(Outcome::Acknowledgement)
                .map_err(Error::Acknowledgement),
            kind => Err(Error::Unsupported(kind)),
        }
    }

    pub fn read_node_work(&self, output: &mut [u8]) -> io::Result<usize> {
        self.owner.read_pending_node_work(output)
    }

    pub fn wait_for_node_work(&self, timeout: Duration) -> bool {
        self.owner.wait_for_node_work(timeout)
    }

    pub(crate) fn remote_node_for_handle(
        &self,
        handle: u32,
    ) -> Result<crate::authority_protocol::NodeToken, reference_table::Error> {
        self.references.remote_node(handle)
    }
}

#[cfg(test)]
mod tests;
