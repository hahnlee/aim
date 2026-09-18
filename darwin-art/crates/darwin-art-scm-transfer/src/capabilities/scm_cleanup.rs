//! Capability cleanup is distinct from native alias retirement (guardian EOF).
use super::{
    Binding, CapabilityError, CapabilityRegistry, DelegationState, DeliveryDisposition,
    TrustedAuthorizer,
};
use crate::TransferKey;

impl CapabilityRegistry {
    /// Trusted service settlement/EOF cleanup of its exact ledger key.
    /// Callers must authenticate receiver/source against the service ledger;
    /// this is deliberately not a guest-facing cancel-by-ticket API.
    pub fn settle_scm_batch(
        &mut self,
        authorizer: &TrustedAuthorizer,
        key: TransferKey,
        disposition: DeliveryDisposition,
    ) -> Result<(), CapabilityError> {
        if authorizer.authority != self.authority || key.authority != self.authority {
            return Err(CapabilityError::WrongAuthority);
        }
        if key.ticket == 0 {
            return Err(CapabilityError::WrongBinding);
        }
        self.delegations.retain(|_, delegation| {
            if !matches!(delegation.record.binding,
                Binding::Scm { transfer_key, .. } if transfer_key == key)
            {
                return true;
            }
            if disposition == DeliveryDisposition::Aborted {
                if let DelegationState::Claimed { holder_id, .. } = delegation.record.state {
                    self.holders.remove(&holder_id);
                }
            }
            false
        });
        self.carriers.retain(|carrier, _| {
            self.holders
                .values()
                .any(|holder| holder.endpoint().carrier == *carrier)
                || self
                    .delegations
                    .values()
                    .any(|entry| entry.record.endpoint.carrier == *carrier)
        });
        Ok(())
    }
}
