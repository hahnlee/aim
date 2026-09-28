//! Resolve only an actual handle0 request through the registered context slot.
use super::*;
use crate::{
    session::transaction_target,
    transaction_request::{Request, Target},
};

#[derive(Debug, PartialEq, Eq)]
pub enum RoutingError {
    NotContextRequest,
    Registry(super::super::Error),
    Connection(connection::Error),
    MissingManager,
    SelfConnection,
    Node(transaction_target::Error),
    Poisoned,
}
impl Registry {
    /// Caller must still enforce transaction permission and sender/thread
    /// lifetime during submission. This resolves resources, not a BR delivery.
    pub fn resolve_context_target(
        &self,
        sender: &Key,
        request: &Request,
    ) -> Result<TargetConnection, RoutingError> {
        if request.target() != Target::Handle(0) {
            return Err(RoutingError::NotContextRequest);
        }
        self.lookup(sender)
            .map_err(RoutingError::Registry)?
            .ensure_open()
            .map_err(RoutingError::Connection)?;
        let context = self.context.lock().map_err(|_| RoutingError::Poisoned)?;
        let manager = context
            .manager
            .as_ref()
            .ok_or(RoutingError::MissingManager)?;
        if manager.key.id == sender.id {
            return Err(RoutingError::SelfConnection);
        }
        let node = Arc::clone(manager._references.node());
        let references = TargetReferences::for_context_node(node).map_err(RoutingError::Node)?;
        // Capture this manager generation, never re-resolve handle0 at commit.
        self.bind_target(&manager.key, references)
            .map_err(RoutingError::Registry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{command::Kind, objects, session::Session, transaction_request};
    fn open(registry: &Registry) -> Key {
        registry
            .register(ConnectionOwner::new(Session::default()))
            .unwrap()
    }
    fn request(kind: Kind, handle: u32) -> Request {
        let mut bytes = kind.word().to_le_bytes().to_vec();
        bytes.resize(4 + kind.payload_size(), 0);
        bytes[4..8].copy_from_slice(&handle.to_le_bytes());
        transaction_request::decode(&bytes).unwrap().0
    }
    fn register(registry: &Registry, key: &Key) {
        let mut bytes = [0; 24];
        bytes[..4].copy_from_slice(&objects::Kind::Binder.tag().to_le_bytes());
        let objects = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
        registry
            .register_context_manager(key, &objects[0], |_| Ok(1000))
            .unwrap();
    }
    #[test]
    fn context_route_keeps_exact_generation_and_rejects_self_missing_foreign() {
        let registry = Registry::default();
        let sender = open(&registry);
        let manager = open(&registry);
        let req = request(Kind::Transaction, 0);
        assert!(matches!(
            registry.resolve_context_target(&sender, &req),
            Err(RoutingError::MissingManager)
        ));
        register(&registry, &manager);
        assert!(matches!(
            registry.resolve_context_target(&manager, &req),
            Err(RoutingError::SelfConnection)
        ));
        let other = Registry::default();
        let foreign = open(&other);
        assert!(matches!(
            registry.resolve_context_target(&foreign, &req),
            Err(RoutingError::Registry(
                super::super::super::Error::ForeignRegistry
            ))
        ));
        let target = registry.resolve_context_target(&sender, &req).unwrap();
        assert_eq!(
            target.commit(|node| Ok(node.pointer())).unwrap().unwrap(),
            0
        );
        registry.close(&manager).unwrap();
        let replacement = open(&registry);
        register(&registry, &replacement);
        assert!(matches!(
            target.commit(|_| Ok(())),
            Err(connection::Error::Closed)
        ));
        assert!(
            registry
                .resolve_context_target(&sender, &req)
                .unwrap()
                .commit(|_| Ok(()))
                .unwrap()
                .is_ok()
        );
    }
    #[test]
    fn normal_handles_and_replies_cannot_enter_context_route() {
        let registry = Registry::default();
        let sender = open(&registry);
        for req in [request(Kind::Transaction, 1), request(Kind::Reply, 0)] {
            assert!(matches!(
                registry.resolve_context_target(&sender, &req),
                Err(RoutingError::NotContextRequest)
            ));
        }
        registry.close(&sender).unwrap();
        assert!(matches!(
            registry.resolve_context_target(&sender, &request(Kind::TransactionSg, 0)),
            Err(RoutingError::Registry(
                super::super::super::Error::UnknownConnection
            ))
        ));
    }
}
