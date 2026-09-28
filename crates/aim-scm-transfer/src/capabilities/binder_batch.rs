//! Trusted Binder deposit/TAKE transitions, separate from native FD ownership.
//! Callers authenticate the source session and actual routed destination. No
//! client request can commit a deposit or authorize its own delivery.
use super::{
    AttributeKind, Binding, CapabilityError, CapabilityRegistry, DelegationRecord, DelegationState,
    TrustedAuthorizer, decode_attributes,
};
use crate::{MAX_PAYLOADS, ProcessEpoch};

#[derive(Clone, Copy, Debug)]
pub struct BinderManifestItem {
    pub ordinal: u64,
    pub object_offset: u64,
    pub attributes: [u8; super::ATTRIBUTES_BYTES],
}

fn transfer_matches(binding: Binding, connection: u64, transfer: u64) -> bool {
    matches!(binding, Binding::Binder { source_connection, transfer: token, .. }
        if source_connection == connection && token == transfer)
}

impl CapabilityRegistry {
    fn binder_authorizer(
        &self,
        authorizer: &TrustedAuthorizer,
        connection: u64,
        transfer: u64,
    ) -> Result<(), CapabilityError> {
        if authorizer.authority != self.authority {
            return Err(CapabilityError::WrongAuthority);
        }
        if connection == 0 || transfer == 0 {
            return Err(CapabilityError::WrongBinding);
        }
        Ok(())
    }

    /// Idempotent source-side cancellation only before a genuine deposit.
    /// Matching by the known binding covers a lost Bind response. A committed
    /// or claimed delivery is never revoked when an export lease is dropped.
    pub fn cancel_pending_binder_binding(
        &mut self,
        source: ProcessEpoch,
        binding: Binding,
    ) -> Result<(), CapabilityError> {
        super::valid_process(source)?;
        if !matches!(binding, Binding::Binder { .. }) || !binding.valid_for(self.authority) {
            return Err(CapabilityError::WrongBinding);
        }
        let found = self
            .delegations
            .values()
            .find(|entry| entry.record.source == source && entry.record.binding == binding)
            .map(|entry| entry.record);
        if let Some(record) = found {
            if record.state == DelegationState::Pending {
                self.delegations.remove(&record.id.get());
                self.retire_if_empty(record.endpoint.carrier);
            }
        }
        Ok(())
    }

    /// Session loss proves only this Binder connection is gone. It does not
    /// revoke committed deliveries or grants on another protocol/connection.
    pub fn cancel_pending_binder_session(
        &mut self,
        authorizer: &TrustedAuthorizer,
        source: ProcessEpoch,
        connection: u64,
    ) -> Result<(), CapabilityError> {
        if authorizer.authority != self.authority {
            return Err(CapabilityError::WrongAuthority);
        }
        super::valid_process(source)?;
        if connection == 0 {
            return Err(CapabilityError::WrongBinding);
        }
        let mut removed: [Option<DelegationRecord>; super::MAX_DELEGATIONS] =
            [None; super::MAX_DELEGATIONS];
        for (slot, entry) in removed.iter_mut().zip(self.delegations.values().filter(|entry|
            entry.record.source == source && entry.record.state == DelegationState::Pending &&
            matches!(entry.record.binding, Binding::Binder { source_connection, .. } if source_connection == connection))) {
            *slot = Some(entry.record);
        }
        for record in removed.into_iter().flatten() {
            self.delegations.remove(&record.id.get());
            self.retire_if_empty(record.endpoint.carrier);
        }
        Ok(())
    }

    /// An absent Binder table key may mean TAKE already consumed it. Cancel
    /// only Pending bindings here, preserving every committed/taken delivery.
    pub fn cancel_pending_binder_transfer(
        &mut self,
        authorizer: &TrustedAuthorizer,
        source: ProcessEpoch,
        connection: u64,
        transfer: u64,
    ) -> Result<(), CapabilityError> {
        self.binder_authorizer(authorizer, connection, transfer)?;
        super::valid_process(source)?;
        let mut removed: [Option<DelegationRecord>; super::MAX_DELEGATIONS] =
            [None; super::MAX_DELEGATIONS];
        for (slot, entry) in removed
            .iter_mut()
            .zip(self.delegations.values().filter(|entry| {
                entry.record.source == source
                    && entry.record.state == DelegationState::Pending
                    && transfer_matches(entry.record.binding, connection, transfer)
            }))
        {
            *slot = Some(entry.record);
        }
        for record in removed.into_iter().flatten() {
            self.delegations.remove(&record.id.get());
            self.retire_if_empty(record.endpoint.carrier);
        }
        Ok(())
    }

    /// Validate the entire immutable FD manifest before changing any entry.
    /// The Binder owner reserves its namespace first and holds its transfer
    /// lock through this operation and allocation-free table installation.
    pub fn commit_binder_deposit(
        &mut self,
        authorizer: &TrustedAuthorizer,
        source: ProcessEpoch,
        connection: u64,
        transfer: u64,
        manifest: &[BinderManifestItem],
    ) -> Result<(), CapabilityError> {
        self.ensure_live()?;
        self.binder_authorizer(authorizer, connection, transfer)?;
        super::valid_process(source)?;
        if self.is_dead(source) {
            return Err(CapabilityError::SourceDead);
        }
        if manifest.len() > MAX_PAYLOADS {
            return Err(CapabilityError::QuotaExceeded);
        }
        let mut records: [Option<DelegationRecord>; MAX_PAYLOADS] = [None; MAX_PAYLOADS];
        for (index, item) in manifest.iter().enumerate() {
            let attrs = decode_attributes(&item.attributes)?;
            if attrs.authority != self.authority {
                return Err(CapabilityError::WrongAuthority);
            }
            if attrs.kind != AttributeKind::Binder {
                return Err(CapabilityError::WrongKind);
            }
            let binding = Binding::Binder {
                source_connection: connection,
                transfer,
                ordinal: item.ordinal,
                object_offset: item.object_offset,
            };
            let record = self.delegation(attrs.delegation)?;
            if record.source != source {
                return Err(CapabilityError::WrongHolder);
            }
            if record.binding != binding {
                return Err(CapabilityError::WrongBinding);
            }
            if record.state != DelegationState::Pending {
                return Err(CapabilityError::WrongState);
            }
            if records[..index].iter().flatten().any(|previous| {
                previous.id == record.id
                    || matches!(previous.binding,
                    Binding::Binder { ordinal, .. } if ordinal == item.ordinal)
            }) {
                return Err(CapabilityError::WrongBinding);
            }
            records[index] = Some(record);
        }
        // Extra, omitted, or duplicated managed attributes cannot silently
        // downgrade a bound descriptor to an ordinary native right.
        let expected = self
            .delegations
            .values()
            .filter(|entry| {
                entry.record.source == source
                    && transfer_matches(entry.record.binding, connection, transfer)
            })
            .count();
        if expected != manifest.len() {
            return Err(CapabilityError::WrongBinding);
        }
        for record in records.into_iter().flatten() {
            self.delegations
                .get_mut(&record.id.get())
                .expect("validated exact batch")
                .record
                .state = DelegationState::Committed { receiver: None };
        }
        Ok(())
    }

    /// Actual TAKE proves both the routed transfer and authenticated receiver.
    /// Preflight every record so a failed last record cannot authorize a prefix.
    pub fn authorize_binder_take(
        &mut self,
        authorizer: &TrustedAuthorizer,
        connection: u64,
        transfer: u64,
        receiver: ProcessEpoch,
    ) -> Result<(), CapabilityError> {
        self.ensure_live()?;
        self.binder_authorizer(authorizer, connection, transfer)?;
        super::valid_process(receiver)?;
        if self.is_dead(receiver) {
            return Err(CapabilityError::ReceiverDead);
        }
        let mut count = 0;
        for entry in self
            .delegations
            .values()
            .filter(|entry| transfer_matches(entry.record.binding, connection, transfer))
        {
            count += 1;
            if count > MAX_PAYLOADS {
                return Err(CapabilityError::QuotaExceeded);
            }
            if entry.record.state != (DelegationState::Committed { receiver: None }) {
                return Err(CapabilityError::WrongState);
            }
        }
        for entry in self
            .delegations
            .values_mut()
            .filter(|entry| transfer_matches(entry.record.binding, connection, transfer))
        {
            entry.record.state = DelegationState::Committed {
                receiver: Some(receiver),
            };
        }
        Ok(())
    }

    /// A genuine claim handler owns this receipt until its confirmation leaves
    /// the daemon. The client may already have released the staged holder after
    /// a lost receipt, so cleanup cannot require that holder still exist.
    pub fn abort_binder_claim_receipt(
        &mut self,
        authorizer: &TrustedAuthorizer,
        claim: super::ClaimedDelivery,
    ) -> Result<(), CapabilityError> {
        if authorizer.authority != self.authority {
            return Err(CapabilityError::WrongAuthority);
        }
        if claim.endpoint.carrier.authority != self.authority
            || claim.grant.endpoint() != claim.endpoint
            || claim.grant.process() != claim.receiver
        {
            return Err(CapabilityError::WrongHolder);
        }
        if self
            .holders
            .get(&claim.grant.id())
            .is_some_and(|holder| holder.grant != claim.grant)
        {
            return Err(CapabilityError::WrongHolder);
        }
        if let Some(entry) = self.delegations.get(&claim.delegation.get()) {
            let record = entry.record;
            if !matches!(record.binding, Binding::Binder { .. })
                || record.endpoint != claim.endpoint
                || record.state
                    != (DelegationState::Claimed {
                        receiver: claim.receiver,
                        holder_id: claim.grant.id(),
                    })
            {
                return Err(CapabilityError::WrongState);
            }
            self.delegations.remove(&claim.delegation.get());
        }
        if let Some(holder) = self.holders.get(&claim.grant.id()) {
            if holder.grant != claim.grant {
                return Err(CapabilityError::WrongHolder);
            }
            self.holders.remove(&claim.grant.id());
        }
        self.retire_if_empty(claim.endpoint.carrier);
        Ok(())
    }

    /// Cleanup is daemon-only. A receiver may discard only an authorized exact
    /// transfer; source/table cleanup uses None after proving pending removal.
    /// Finished native claims already removed their delegation and retain their
    /// own Description holder; those are closed by native failure ownership.
    pub fn abort_binder_transfer(
        &mut self,
        authorizer: &TrustedAuthorizer,
        connection: u64,
        transfer: u64,
        receiver: Option<ProcessEpoch>,
    ) -> Result<(), CapabilityError> {
        self.binder_authorizer(authorizer, connection, transfer)?;
        for entry in self
            .delegations
            .values()
            .filter(|entry| transfer_matches(entry.record.binding, connection, transfer))
        {
            if let Some(peer) = receiver {
                if !matches!(entry.record.state,
                    DelegationState::Committed { receiver: Some(expected) } |
                    DelegationState::Claimed { receiver: expected, .. } if expected == peer)
                {
                    return Err(CapabilityError::WrongHolder);
                }
            }
        }
        let mut removed: [Option<DelegationRecord>; super::MAX_DELEGATIONS] =
            [None; super::MAX_DELEGATIONS];
        let mut count = 0;
        for entry in self
            .delegations
            .values()
            .filter(|entry| transfer_matches(entry.record.binding, connection, transfer))
        {
            if count == removed.len() {
                return Err(CapabilityError::QuotaExceeded);
            }
            removed[count] = Some(entry.record);
            count += 1;
        }
        for record in removed.into_iter().flatten() {
            if let DelegationState::Claimed { holder_id, .. } = record.state {
                self.holders.remove(&holder_id);
            }
            self.delegations.remove(&record.id.get());
            self.retire_if_empty(record.endpoint.carrier);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CapabilityAttributes, encode_attributes};
    use super::*;
    use crate::{AuthorityEpoch, Side};
    fn peer(pid: u32) -> ProcessEpoch {
        ProcessEpoch {
            pid,
            instance: pid as u128 + 100,
        }
    }
    fn binding(ordinal: u64) -> Binding {
        Binding::Binder {
            source_connection: 7,
            transfer: 8,
            ordinal,
            object_offset: ordinal * 24,
        }
    }
    fn setup() -> (CapabilityRegistry, Vec<BinderManifestItem>) {
        let mut registry = CapabilityRegistry::new(AuthorityEpoch { instance: 12 }).unwrap();
        let pair = registry.register_pair(peer(1)).unwrap();
        let mut manifest = Vec::new();
        for (ordinal, holder) in [(0, pair.holder_a), (1, pair.holder_b)] {
            let delegation = registry
                .prepare_delegation(holder, binding(ordinal))
                .unwrap();
            manifest.push(BinderManifestItem {
                ordinal,
                object_offset: ordinal * 24,
                attributes: encode_attributes(CapabilityAttributes {
                    kind: AttributeKind::Binder,
                    authority: registry.authority(),
                    delegation,
                }),
            });
        }
        (registry, manifest)
    }
    #[test]
    fn malformed_last_item_or_missing_descriptor_never_commits_a_prefix() {
        let (mut registry, manifest) = setup();
        let auth = registry.trusted_authorizer();
        let mut bad = manifest.clone();
        bad[1].object_offset += 1;
        assert!(
            registry
                .commit_binder_deposit(&auth, peer(1), 7, 8, &bad)
                .is_err()
        );
        assert!(
            registry
                .commit_binder_deposit(&auth, peer(1), 7, 8, &manifest[..1])
                .is_err()
        );
        for item in &manifest {
            let id = decode_attributes(&item.attributes).unwrap().delegation;
            assert_eq!(
                registry.delegation(id).unwrap().state,
                DelegationState::Pending
            );
        }
        registry
            .commit_binder_deposit(&auth, peer(1), 7, 8, &manifest)
            .unwrap();
        assert!(
            registry
                .commit_binder_deposit(&auth, peer(1), 7, 8, &manifest)
                .is_err()
        );
    }
    #[test]
    fn source_lease_drop_preserves_committed_delivery_and_original_endpoint_side() {
        let (mut registry, manifest) = setup();
        let auth = registry.trusted_authorizer();
        registry
            .commit_binder_deposit(&auth, peer(1), 7, 8, &manifest)
            .unwrap();
        registry
            .cancel_pending_binder_binding(peer(1), binding(0))
            .unwrap();
        registry.source_died(peer(1)).unwrap();
        registry
            .authorize_binder_take(&auth, 7, 8, peer(2))
            .unwrap();
        assert!(
            registry
                .claim(peer(3), &manifest[0].attributes, binding(0))
                .is_err()
        );
        let a = registry
            .claim(peer(2), &manifest[0].attributes, binding(0))
            .unwrap();
        let b = registry
            .claim(peer(2), &manifest[1].attributes, binding(1))
            .unwrap();
        assert_eq!(a.endpoint.side, Side::A);
        assert_eq!(b.endpoint.side, Side::B);
        registry.finish(a.delegation, a.grant).unwrap();
        registry
            .abort_binder_transfer(&auth, 7, 8, Some(peer(2)))
            .unwrap();
        assert_eq!(registry.holder_count(), 1);
        registry.release_holder(a.grant).unwrap();
        assert_eq!(registry.carrier_count(), 0);
    }
    #[test]
    fn lost_bind_reply_cancels_by_exact_binding_and_duplicate_bind_is_rejected() {
        let (mut registry, _) = setup();
        let pair = registry.register_pair(peer(1)).unwrap();
        assert!(
            registry
                .prepare_delegation(pair.holder_a, binding(0))
                .is_err()
        );
        registry
            .cancel_pending_binder_binding(peer(1), binding(0))
            .unwrap();
        registry
            .cancel_pending_binder_binding(peer(1), binding(0))
            .unwrap();
        assert_eq!(registry.delegation_count(), 1);
        registry
            .prepare_delegation(pair.holder_a, binding(0))
            .unwrap();
    }
    #[test]
    fn receipt_rollback_cleans_a_claim_even_after_the_client_released_its_holder() {
        let (mut registry, manifest) = setup();
        let auth = registry.trusted_authorizer();
        registry
            .commit_binder_deposit(&auth, peer(1), 7, 8, &manifest)
            .unwrap();
        registry
            .authorize_binder_take(&auth, 7, 8, peer(2))
            .unwrap();
        let claim = registry
            .claim(peer(2), &manifest[0].attributes, binding(0))
            .unwrap();
        registry.release_holder(claim.grant).unwrap();
        registry.abort_binder_claim_receipt(&auth, claim).unwrap();
        registry.abort_binder_claim_receipt(&auth, claim).unwrap();
        assert_eq!(registry.delegation_count(), 1);
    }
    #[test]
    fn sealed_registry_still_discards_pending_and_authorized_binder_entries() {
        let (mut registry, manifest) = setup();
        let auth = registry.trusted_authorizer();
        registry
            .commit_binder_deposit(&auth, peer(1), 7, 8, &manifest)
            .unwrap();
        registry
            .authorize_binder_take(&auth, 7, 8, peer(2))
            .unwrap();
        registry.sealed = true;
        registry
            .abort_binder_transfer(&auth, 7, 8, Some(peer(2)))
            .unwrap();
        assert_eq!(registry.delegation_count(), 0);
    }
    #[test]
    fn wrong_receiver_discard_does_not_revoke_the_actual_delivery() {
        let (mut registry, manifest) = setup();
        let auth = registry.trusted_authorizer();
        registry
            .commit_binder_deposit(&auth, peer(1), 7, 8, &manifest)
            .unwrap();
        registry
            .authorize_binder_take(&auth, 7, 8, peer(2))
            .unwrap();
        assert!(
            registry
                .abort_binder_transfer(&auth, 7, 8, Some(peer(3)))
                .is_err()
        );
        assert_eq!(registry.delegation_count(), 2);
        registry
            .abort_binder_transfer(&auth, 7, 8, Some(peer(2)))
            .unwrap();
        assert_eq!(registry.delegation_count(), 0);
    }
}
