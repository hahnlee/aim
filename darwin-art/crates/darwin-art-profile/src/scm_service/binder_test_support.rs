//! TEST-ONLY ledger access for cross-owner policy/real-FD regression tests.
use super::{ScmService, binder_capabilities::epoch};
use darwin_art_binder_device::routing_authority::PeerIdentity;
use darwin_art_scm_transfer::capabilities::{DelegationId, DelegationRecord, RegisteredPair};
impl ScmService {
    pub(crate) fn test_pair(&self, peer: PeerIdentity) -> RegisteredPair {
        self.owner
            .lock()
            .unwrap()
            .register_pair(epoch(peer))
            .unwrap()
    }
    pub(crate) fn test_delegation(&self, id: DelegationId) -> DelegationRecord {
        self.owner.lock().unwrap().registry.delegation(id).unwrap()
    }
    pub(crate) fn test_delegation_count(&self) -> usize {
        self.owner.lock().unwrap().registry.delegation_count()
    }
}
