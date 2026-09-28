//! Preallocated admission transaction for a daemon's exact managed-FD ledger.
use super::holders::HolderRecord;
use super::{
    Binding, CapabilityError, CapabilityRegistry, ClaimedDelivery, DelegationId, DelegationRecord,
    DelegationState, HolderGrant, MAX_HOLDERS, RANDOM_RETRIES, TrustedAuthorizer,
};
use crate::{AuthorityEpoch, MAX_PAYLOADS, ProcessEpoch, TransferKey};

/// Private staged grants cannot authenticate until the whole batch commits.
/// Keep the service lock from preflight through core admission and this commit.
pub struct ClaimReservation {
    authority: AuthorityEpoch,
    receiver: ProcessEpoch,
    entries: Vec<(DelegationRecord, Option<HolderGrant>)>,
    results: Vec<ClaimedDelivery>,
}

impl CapabilityRegistry {
    pub fn prepare_scm_claim(
        &mut self,
        authorizer: &TrustedAuthorizer,
        receiver: ProcessEpoch,
        key: TransferKey,
        managed: &[(u64, DelegationId)],
        publish_ordinals: &[u64],
    ) -> Result<ClaimReservation, CapabilityError> {
        self.ensure_live()?;
        super::valid_process(receiver)?;
        if self.is_dead(receiver) {
            return Err(CapabilityError::ReceiverDead);
        }
        if authorizer.authority != self.authority || key.authority != self.authority {
            return Err(CapabilityError::WrongAuthority);
        }
        if key.ticket == 0 || managed.len() > MAX_PAYLOADS || publish_ordinals.len() > managed.len()
        {
            return Err(CapabilityError::WrongBinding);
        }
        for (index, ordinal) in publish_ordinals.iter().enumerate() {
            if publish_ordinals[..index].contains(ordinal)
                || !managed.iter().any(|(candidate, _)| candidate == ordinal)
            {
                return Err(CapabilityError::WrongBinding);
            }
        }
        let mut entries = Vec::new();
        let mut results = Vec::new();
        entries
            .try_reserve(managed.len())
            .map_err(|_| CapabilityError::QuotaExceeded)?;
        results
            .try_reserve(publish_ordinals.len())
            .map_err(|_| CapabilityError::QuotaExceeded)?;
        for (index, (ordinal, id)) in managed.iter().enumerate() {
            if *ordinal >= MAX_PAYLOADS as u64
                || managed[..index]
                    .iter()
                    .any(|(earlier, previous)| earlier == ordinal || previous == id)
            {
                return Err(CapabilityError::WrongBinding);
            }
            let record = self.delegation(*id)?;
            if record.binding
                != (Binding::Scm {
                    transfer_key: key,
                    ordinal: *ordinal,
                })
            {
                return Err(CapabilityError::WrongBinding);
            }
            if record.state != (DelegationState::Committed { receiver: None }) {
                return Err(CapabilityError::WrongState);
            }
            entries.push((record, None));
        }
        if self
            .holders
            .len()
            .checked_add(publish_ordinals.len())
            .filter(|count| *count <= MAX_HOLDERS)
            .is_none()
        {
            return Err(CapabilityError::QuotaExceeded);
        }
        self.holders
            .try_reserve(publish_ordinals.len())
            .map_err(|_| CapabilityError::QuotaExceeded)?;
        for (record, staged) in &mut entries {
            let Binding::Scm { ordinal, .. } = record.binding else {
                unreachable!()
            };
            if !publish_ordinals.contains(&ordinal) {
                continue;
            }
            let mut fresh = None;
            for _ in 0..RANDOM_RETRIES {
                let id = self.fresh_id(true)?;
                if !results
                    .iter()
                    .any(|result: &ClaimedDelivery| result.grant.id() == id)
                {
                    fresh = Some(id);
                    break;
                }
            }
            let grant = HolderGrant {
                authority: self.authority,
                id: fresh.ok_or(CapabilityError::IdentifierExhausted)?,
                process: receiver,
                endpoint: record.endpoint,
            };
            *staged = Some(grant);
            results.push(ClaimedDelivery {
                delegation: record.id,
                grant,
                endpoint: record.endpoint,
                receiver,
            });
        }
        Ok(ClaimReservation {
            authority: self.authority,
            receiver,
            entries,
            results,
        })
    }

    /// No allocation after validation. Under an unchanged service lock the
    /// checks cannot fail after a successful reservation/core preflight.
    /// Stale/replayed reservations fail before changing any registry entry.
    pub fn commit_scm_claim(
        &mut self,
        reserved: ClaimReservation,
    ) -> Result<Vec<ClaimedDelivery>, CapabilityError> {
        self.ensure_live()?;
        if reserved.authority != self.authority {
            return Err(CapabilityError::WrongAuthority);
        }
        if self.is_dead(reserved.receiver) {
            return Err(CapabilityError::ReceiverDead);
        }
        if self
            .holders
            .len()
            .checked_add(reserved.results.len())
            .filter(|count| *count <= MAX_HOLDERS && *count <= self.holders.capacity())
            .is_none()
        {
            return Err(CapabilityError::QuotaExceeded);
        }
        for (record, grant) in &reserved.entries {
            if self.delegation(record.id)? != *record {
                return Err(CapabilityError::WrongState);
            }
            if grant.is_some_and(|grant| self.holders.contains_key(&grant.id())) {
                return Err(CapabilityError::WrongHolder);
            }
        }
        for (record, grant) in reserved.entries {
            if let Some(grant) = grant {
                self.holders.insert(grant.id(), HolderRecord { grant });
                self.delegations
                    .get_mut(&record.id.get())
                    .expect("validated record")
                    .record
                    .state = DelegationState::Claimed {
                    receiver: reserved.receiver,
                    holder_id: grant.id(),
                };
            } else {
                self.delegations.remove(&record.id.get());
                self.retire_if_empty(record.endpoint.carrier);
            }
        }
        Ok(reserved.results)
    }
}
