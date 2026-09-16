//! Immutable selection metadata contains weak identities, never owning graph
//! references. Keeping metadata for a child cannot keep a parent mapping live.
use super::*;
pub(super) struct SelectionMetadata {
    pub indices: HashMap<String, usize>,
    pub group_roots: HashMap<String, String>,
    pub identities: Vec<mappings::MappingIdentity>,
}
