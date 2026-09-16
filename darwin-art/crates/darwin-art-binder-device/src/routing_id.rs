//! Opaque authority identities. Raw process IDs and object addresses are never
//! routing keys.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ConnectionId(u64);

impl ConnectionId {
    pub(crate) fn next(previous: u64) -> Option<(Self, u64)> {
        let value = previous.checked_add(1)?;
        Some((Self(value), value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LocalNodeId(u64);

impl LocalNodeId {
    pub(crate) fn next(previous: u64) -> Option<(Self, u64)> {
        let value = previous.checked_add(1)?;
        Some((Self(value), value))
    }

    pub(crate) fn from_nonzero(value: u64) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }

    pub(crate) fn get(self) -> u64 {
        self.0
    }
}

/// Authority-visible Binder node identity. The process-local pointer and cookie
/// never cross this boundary; the owning connection supplies the namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct NodeId {
    owner: ConnectionId,
    local: LocalNodeId,
}

impl NodeId {
    pub(crate) fn new(owner: ConnectionId, local: LocalNodeId) -> Self {
        Self { owner, local }
    }

    pub(crate) fn owner(self) -> ConnectionId {
        self.owner
    }
}
