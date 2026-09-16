//! Transaction node references, not thread-stack or target-process ownership.
use super::*;
use crate::{
    node_owner::{HoldKind, LocalHold},
    transaction_request::{Request, Target},
};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    ContextManagerRequired,
    ReplyStackRequired,
    Reference(reference_table::Error),
    DeadTarget,
    SelfTransaction,
    HoldFailed(i32),
    RemoteTarget,
}

pub enum ResolvedTransactionTarget {
    Local(TargetReferences),
    Remote(crate::authority_protocol::NodeToken),
}

/// Keeps both actual strong demand and temporary node demand through preparation.
/// It does NOT keep a process alive. The device registry must separately acquire
/// the target process/connection and serialize delivery against its teardown.
pub struct TargetReferences {
    strong: LocalHold,
    _temporary: LocalHold,
}
impl TargetReferences {
    pub(crate) fn for_context_node(node: Arc<Node>) -> Result<Self, Error> {
        if !node.owner_alive() {
            return Err(Error::DeadTarget);
        }
        let temporary = node
            .hold(HoldKind::Temporary)
            .map_err(|e| Error::HoldFailed(e.raw_os_error().unwrap_or(libc::EIO)))?;
        let strong = node
            .hold(HoldKind::Strong)
            .map_err(|e| Error::HoldFailed(e.raw_os_error().unwrap_or(libc::EIO)))?;
        Ok(Self {
            strong,
            _temporary: temporary,
        })
    }
    pub fn node(&self) -> &Node {
        self.strong.node()
    }
}

impl Session {
    /// Resolve only ordinary target handles in this authenticated sender session.
    /// Context-manager selection and replies require their distinct authorities.
    pub fn resolve_transaction_target(&self, request: &Request) -> Result<TargetReferences, Error> {
        match self.resolve_transaction_route(request)? {
            ResolvedTransactionTarget::Local(references) => Ok(references),
            ResolvedTransactionTarget::Remote(_) => Err(Error::RemoteTarget),
        }
    }

    pub fn resolve_transaction_route(
        &self,
        request: &Request,
    ) -> Result<ResolvedTransactionTarget, Error> {
        let handle = match request.target() {
            Target::Reply => return Err(Error::ReplyStackRequired),
            Target::Handle(handle) => handle,
        };
        let resolved = match self.references.resolve(handle, Strength::Strong) {
            Err(reference_table::Error::UnknownHandle) if handle == 0 => {
                return Err(Error::ContextManagerRequired);
            }
            result => result.map_err(Error::Reference)?,
        };
        let temporary = match resolved {
            reference_table::ResolvedTarget::Local(temporary) => temporary,
            reference_table::ResolvedTarget::Remote(node) => {
                return Ok(ResolvedTransactionTarget::Remote(node));
            }
        };
        let node = temporary.node();
        if !node.owner_alive() {
            return Err(Error::DeadTarget);
        }
        if self.owner.owns(node) {
            return Err(Error::SelfTransaction);
        }
        let strong = node
            .hold(HoldKind::Strong)
            .map_err(|e| Error::HoldFailed(e.raw_os_error().unwrap_or(libc::EIO)))?;
        Ok(ResolvedTransactionTarget::Local(TargetReferences {
            strong,
            _temporary: temporary,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{objects, transaction_request};
    fn node(owner: &mut Session) -> Arc<Node> {
        let mut bytes = [0; 24];
        bytes[..4].copy_from_slice(&objects::Kind::Binder.tag().to_le_bytes());
        bytes[8..16].copy_from_slice(&123u64.to_le_bytes());
        let objects = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
        owner.resolve_local(&objects[0]).unwrap()
    }
    fn request(kind: Kind, handle: u32) -> Request {
        let mut bytes = kind.word().to_le_bytes().to_vec();
        bytes.resize(4 + kind.payload_size(), 0);
        bytes[4..8].copy_from_slice(&handle.to_le_bytes());
        transaction_request::decode(&bytes).unwrap().0
    }
    #[test]
    fn prepared_target_survives_last_sender_handle_release() {
        let mut owner = Session::default();
        let node = node(&mut owner);
        let mut sender = Session::default();
        sender.retain_transferred(node, Strength::Strong).unwrap();
        let mut output = [0; 40];
        assert_eq!(owner.read_node_work(&mut output).unwrap(), 40);
        owner.owner.acknowledge(123, 0, Strength::Weak).unwrap();
        owner.owner.acknowledge(123, 0, Strength::Strong).unwrap();
        let target = sender
            .resolve_transaction_target(&request(Kind::Transaction, 1))
            .unwrap();
        assert_eq!(target.node().pointer(), 123);
        drop(sender);
        assert_eq!(owner.read_node_work(&mut output).unwrap(), 0);
        drop(target);
        assert_eq!(owner.read_node_work(&mut output).unwrap(), 40);
        assert_eq!(
            u32::from_le_bytes(output[..4].try_into().unwrap()),
            0x80107209
        );
        assert_eq!(
            u32::from_le_bytes(output[20..24].try_into().unwrap()),
            0x8010720a
        );
    }
    #[test]
    fn rejects_weak_dead_self_and_routes_requiring_other_authority() {
        let mut owner = Session::default();
        let node = node(&mut owner);
        let mut sender = Session::default();
        sender
            .retain_transferred(Arc::clone(&node), Strength::Weak)
            .unwrap();
        let req = request(Kind::Transaction, 1);
        assert!(matches!(
            sender.resolve_transaction_target(&req),
            Err(Error::Reference(reference_table::Error::StrongRequired))
        ));
        assert!(matches!(
            sender.resolve_transaction_target(&request(Kind::Transaction, 99)),
            Err(Error::Reference(reference_table::Error::UnknownHandle))
        ));
        assert!(matches!(
            sender.resolve_transaction_target(&request(Kind::Transaction, 0)),
            Err(Error::ContextManagerRequired)
        ));
        assert!(matches!(
            sender.resolve_transaction_target(&request(Kind::Reply, 1)),
            Err(Error::ReplyStackRequired)
        ));
        owner
            .retain_transferred(Arc::clone(&node), Strength::Strong)
            .unwrap();
        assert!(matches!(
            owner.resolve_transaction_target(&req),
            Err(Error::SelfTransaction)
        ));
        sender.retain_transferred(node, Strength::Strong).unwrap();
        drop(owner);
        assert!(matches!(
            sender.resolve_transaction_target(&req),
            Err(Error::DeadTarget)
        ));
    }

    #[test]
    fn authority_nodes_resolve_as_typed_remote_routes() {
        let connection = crate::authority_protocol::ConnectionToken::from_nonzero(4).unwrap();
        let context = crate::authority_protocol::NodeToken::new(
            connection,
            crate::authority_protocol::LocalNodeToken::from_nonzero(6).unwrap(),
        );
        let ordinary = crate::authority_protocol::NodeToken::new(
            connection,
            crate::authority_protocol::LocalNodeToken::from_nonzero(7).unwrap(),
        );
        let mut session = Session::default();
        assert_eq!(
            session
                .retain_remote_context_manager(context, Strength::Strong)
                .unwrap()
                .0,
            0
        );
        assert_eq!(
            session.retain_remote(ordinary, Strength::Strong).unwrap().0,
            1
        );
        assert!(matches!(
            session.resolve_transaction_route(&request(Kind::Transaction, 0)),
            Ok(ResolvedTransactionTarget::Remote(node)) if node == context
        ));
        assert!(matches!(
            session.resolve_transaction_route(&request(Kind::Transaction, 1)),
            Ok(ResolvedTransactionTarget::Remote(node)) if node == ordinary
        ));
        assert!(matches!(
            session.resolve_transaction_target(&request(Kind::Transaction, 1)),
            Err(Error::RemoteTarget)
        ));
    }
}
