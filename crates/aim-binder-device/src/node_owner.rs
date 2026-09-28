//! Per-owner Binder node identity. Arc retains host metadata only; it is NOT
//! an Android strong/weak reference count or a substitute for BR_* callbacks.
use crate::{object_fields::Fields, objects::Object};
mod lifecycle;
mod work_queue;
pub use lifecycle::{ContextManagerRefs, HoldKind, LocalHold};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

struct OwnerState {
    alive: AtomicBool,
    work: work_queue::WorkQueue,
    signal: Mutex<Option<Arc<crate::work_signal::Signal>>>,
    routing_owner: OnceLock<crate::routing_id::ConnectionId>,
}

pub struct Node {
    owner: Arc<OwnerState>,
    local_id: crate::routing_id::LocalNodeId,
    pointer: u64,
    cookie: u64,
    initial_flags: u32,
    lifecycle: Mutex<lifecycle::Lifecycle>,
}
impl Node {
    pub fn pointer(&self) -> u64 {
        self.pointer
    }
    pub fn cookie(&self) -> u64 {
        self.cookie
    }
    /// Original creation flags; interpreting scheduling/security flags is separate.
    pub fn initial_flags(&self) -> u32 {
        self.initial_flags
    }
    pub fn local_token(&self) -> crate::authority_protocol::LocalNodeToken {
        crate::authority_protocol::LocalNodeToken::from_nonzero(self.local_id.get())
            .expect("allocated local node IDs are nonzero")
    }
    pub fn owner_alive(&self) -> bool {
        self.owner.alive.load(Ordering::Acquire)
    }
    pub(crate) fn owner_connection(&self) -> Option<crate::routing_id::ConnectionId> {
        self.owner.routing_owner.get().copied()
    }
    pub(crate) fn routing_id(&self) -> Option<crate::routing_id::NodeId> {
        self.owner_connection()
            .map(|owner| crate::routing_id::NodeId::new(owner, self.local_id))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    NotLocalNode,
    CookieMismatch,
    OutOfMemory,
    NodeIdsExhausted,
    UnknownNode,
    NoPendingAcknowledgement,
}

/// A session must own exactly one table for its process/device identity. The
/// table is not Clone and takes no sender-supplied PID as an identity authority.
pub struct NodeOwner {
    state: Arc<OwnerState>,
    nodes: HashMap<u64, Arc<Node>>,
    nodes_by_local: HashMap<crate::routing_id::LocalNodeId, Arc<Node>>,
    next_node: u64,
}
impl Default for NodeOwner {
    fn default() -> Self {
        Self::new()
    }
}
impl NodeOwner {
    pub fn new() -> Self {
        Self {
            state: Arc::new(OwnerState {
                alive: AtomicBool::new(true),
                work: work_queue::WorkQueue::default(),
                signal: Mutex::new(None),
                routing_owner: OnceLock::new(),
            }),
            nodes: HashMap::new(),
            nodes_by_local: HashMap::new(),
            next_node: 0,
        }
    }
    /// Resolves a structurally validated local node object within this owner.
    /// Strong and weak transfers share identity; cookie changes are rejected.
    pub fn resolve(&mut self, object: &Object<'_>) -> Result<Arc<Node>, Error> {
        let Fields::Node {
            flags,
            pointer,
            cookie,
            ..
        } = object.fields()
        else {
            return Err(Error::NotLocalNode);
        };
        if let Some(node) = self.nodes.get(&pointer) {
            if node.cookie != cookie {
                return Err(Error::CookieMismatch);
            }
            return Ok(Arc::clone(node));
        }
        self.nodes.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
        self.nodes_by_local
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        let (local_id, serial) =
            crate::routing_id::LocalNodeId::next(self.next_node).ok_or(Error::NodeIdsExhausted)?;
        let node = Arc::new(Node {
            owner: Arc::clone(&self.state),
            local_id,
            pointer,
            cookie,
            initial_flags: flags,
            lifecycle: Mutex::new(lifecycle::Lifecycle::default()),
        });
        self.nodes.insert(pointer, Arc::clone(&node));
        self.nodes_by_local.insert(local_id, Arc::clone(&node));
        self.next_node = serial;
        Ok(node)
    }

    pub fn lookup_local(
        &self,
        token: crate::authority_protocol::LocalNodeToken,
    ) -> Result<Arc<Node>, Error> {
        let local =
            crate::routing_id::LocalNodeId::from_nonzero(token.get()).ok_or(Error::UnknownNode)?;
        self.nodes_by_local
            .get(&local)
            .cloned()
            .ok_or(Error::UnknownNode)
    }
    pub fn owns(&self, node: &Node) -> bool {
        Arc::ptr_eq(&self.state, &node.owner)
    }

    pub(crate) fn attach_work_signal(&self, signal: Arc<crate::work_signal::Signal>) {
        *self
            .state
            .signal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(signal);
    }

    pub(crate) fn bind_routing_id(&self, id: crate::routing_id::ConnectionId) -> Result<(), ()> {
        self.state.routing_owner.set(id).map_err(|_| ())
    }
}
impl Drop for NodeOwner {
    fn drop(&mut self) {
        self.state.alive.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{self, Kind};
    fn bytes(kind: Kind, flags: u32, cookie: u64) -> [u8; 24] {
        let mut bytes = [0; 24];
        bytes[..4].copy_from_slice(&kind.tag().to_le_bytes());
        bytes[4..8].copy_from_slice(&flags.to_le_bytes());
        bytes[8..16].copy_from_slice(&123u64.to_le_bytes());
        bytes[16..24].copy_from_slice(&cookie.to_le_bytes());
        bytes
    }
    #[test]
    fn repeated_strong_weak_exports_share_node_but_not_owner_namespace() {
        let mut first = NodeOwner::new();
        let mut second = NodeOwner::new();
        let data = bytes(Kind::Binder, 0x100, 456);
        let objects = objects::validate(&data, &0u64.to_le_bytes()).unwrap();
        let node = first.resolve(&objects[0]).unwrap();
        let foreign = second.resolve(&objects[0]).unwrap();
        assert!(!Arc::ptr_eq(&node, &foreign));
        assert!(first.owns(&node));
        assert!(!first.owns(&foreign));
        let weak = bytes(Kind::WeakBinder, 0, 456);
        let objects = objects::validate(&weak, &0u64.to_le_bytes()).unwrap();
        assert!(Arc::ptr_eq(&node, &first.resolve(&objects[0]).unwrap()));
        assert_eq!(node.initial_flags(), 0x100);
        assert!(Arc::ptr_eq(
            &node,
            &first.lookup_local(node.local_token()).unwrap()
        ));
        assert_eq!((node.pointer(), node.cookie()), (123, 456));
        drop(first);
        assert!(!node.owner_alive());
        assert!(foreign.owner_alive());
    }
    #[test]
    fn cookie_mismatch_and_non_node_cannot_rebind_identity() {
        let mut owner = NodeOwner::new();
        let original = bytes(Kind::Binder, 1, 456);
        let object = objects::validate(&original, &0u64.to_le_bytes()).unwrap();
        let node = owner.resolve(&object[0]).unwrap();
        let changed = bytes(Kind::Binder, 2, 789);
        let object = objects::validate(&changed, &0u64.to_le_bytes()).unwrap();
        assert!(matches!(
            owner.resolve(&object[0]),
            Err(Error::CookieMismatch)
        ));
        let handle = bytes(Kind::Handle, 1, 456);
        let object = objects::validate(&handle, &0u64.to_le_bytes()).unwrap();
        assert!(matches!(
            owner.resolve(&object[0]),
            Err(Error::NotLocalNode)
        ));
        assert_eq!(node.cookie(), 456);
        assert_eq!(node.initial_flags(), 1);
    }

    #[test]
    fn node_identity_exhaustion_never_publishes_a_partial_node() {
        let mut owner = NodeOwner::new();
        owner.next_node = u64::MAX;
        let data = bytes(Kind::Binder, 0, 456);
        let objects = objects::validate(&data, &0u64.to_le_bytes()).unwrap();
        assert!(matches!(
            owner.resolve(&objects[0]),
            Err(Error::NodeIdsExhausted)
        ));
        assert!(owner.nodes.is_empty());
    }
}
